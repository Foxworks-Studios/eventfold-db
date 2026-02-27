//! Storage engine for EventfoldDB.
//!
//! This module owns the append-only log file and the in-memory index. It provides
//! methods for opening (or creating) the store, appending events with optimistic
//! concurrency, and reading events by stream or globally.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, RwLock};

use uuid::Uuid;

use crate::codec::{self, DecodeOutcome};
use crate::error::Error;
use crate::types::{
    ExpectedVersion, MAX_EVENT_SIZE, MAX_EVENT_TYPE_LEN, ProposedEvent, RecordedEvent,
};

/// Size of the file header in bytes (magic + format version).
const HEADER_SIZE: usize = 8;

/// Check whether a valid batch header exists in `data` after byte offset `start`.
///
/// Scans forward one byte at a time from `start + 1` through the end of the
/// buffer, looking for the `BATCH_HEADER_MAGIC` bytes followed by a decodable
/// batch header. Returns `true` if found, indicating mid-file corruption
/// (the corrupt region is not at the tail).
fn has_valid_batch_after(data: &[u8], start: usize) -> bool {
    for probe in (start + 1)..data.len() {
        if let Ok(DecodeOutcome::Complete { .. }) = codec::decode_batch_header(&data[probe..]) {
            return true;
        }
    }
    false
}

/// Truncate the log file to a given offset, fsync, and return a `Store` with
/// the events recovered so far.
///
/// This is the common recovery path for all partial/corrupt batch scenarios:
/// incomplete header, incomplete record, missing footer, or CRC mismatch.
///
/// # Arguments
///
/// * `path` - Path to the log file.
/// * `truncate_to` - Byte offset to truncate the file to.
/// * `events` - Events recovered from prior complete batches.
/// * `streams` - Stream index recovered from prior complete batches.
///
/// # Returns
///
/// A `Store` with the recovered events and the file truncated.
///
/// # Errors
///
/// Returns [`Error::Io`] if the file cannot be opened or truncated.
fn truncate_and_return(
    path: &Path,
    truncate_to: usize,
    events: Vec<RecordedEvent>,
    streams: HashMap<Uuid, Vec<u64>>,
) -> Result<Store, Error> {
    let file = OpenOptions::new().read(true).write(true).open(path)?;
    file.set_len(truncate_to as u64)?;
    file.sync_all()?;

    Ok(Store {
        file,
        log: Arc::new(RwLock::new(EventLog { events, streams })),
    })
}

/// Thread-safe, read-optimized view of the event log.
///
/// Holds the two in-memory index structures: the global event log and the
/// per-stream index of global positions. Wrapped in `Arc<RwLock<EventLog>>`
/// inside `Store` so that concurrent read handlers can acquire a read lock
/// while the writer task holds an exclusive write lock for index updates.
#[derive(Debug)]
pub struct EventLog {
    /// Global event log. Append-only -- new events are pushed to the end.
    /// Index `i` = event at global position `i`.
    pub events: Vec<RecordedEvent>,
    /// Stream index. Maps stream ID to list of global positions.
    /// Index `j` in the vec = event at stream version `j`.
    pub streams: HashMap<Uuid, Vec<u64>>,
}

/// Core storage engine that manages the append-only log file and in-memory index.
///
/// The `Store` owns the file handle for the log file and a shared
/// `Arc<RwLock<EventLog>>` that holds the in-memory index structures.
///
/// All writes go through `append()`, which validates concurrency, serializes
/// records to disk, fsyncs, then acquires the write lock to update the index.
/// Reads acquire a read lock and go directly to the in-memory index with no
/// disk I/O. The `Arc<RwLock<EventLog>>` can be cloned via `Store::log()` for
/// use by `ReadIndex` handles.
pub struct Store {
    /// Append-only log file handle.
    file: File,
    /// Shared in-memory event log, protected by a read-write lock.
    log: Arc<RwLock<EventLog>>,
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

            // Fsync the parent directory so the new file's directory entry is
            // durable. Without this, a crash between file creation and the OS
            // flushing the directory entry could leave the file inaccessible.
            let parent = path
                .parent()
                .expect("log path must have a parent directory");
            let dir_handle = File::open(parent)?;
            dir_handle.sync_all()?;

            return Ok(Store {
                file,
                log: Arc::new(RwLock::new(EventLog {
                    events: Vec::new(),
                    streams: HashMap::new(),
                })),
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

        // Decode batches sequentially from offset HEADER_SIZE.
        // Each batch is: BatchHeader (16 bytes) + N records + BatchFooter (8 bytes).
        let mut events = Vec::new();
        let mut streams: HashMap<Uuid, Vec<u64>> = HashMap::new();
        let mut offset = HEADER_SIZE;

        loop {
            let remaining = &data[offset..];
            if remaining.is_empty() {
                break;
            }

            let batch_start_offset = offset;

            // Step 1: Decode batch header.
            let header = match codec::decode_batch_header(remaining) {
                Ok(DecodeOutcome::Complete { value, consumed }) => {
                    offset += consumed;
                    value
                }
                Ok(DecodeOutcome::Incomplete) => {
                    // Trailing partial header -- truncate to batch start.
                    tracing::warn!(
                        batch_start_offset,
                        valid_events = events.len(),
                        "truncating trailing partial batch header at byte offset \
                         {batch_start_offset}"
                    );
                    return truncate_and_return(path, batch_start_offset, events, streams);
                }
                Err(Error::CorruptRecord { .. }) => {
                    // Bad magic at this offset. Check if valid data follows.
                    if has_valid_batch_after(&data, batch_start_offset) {
                        return Err(Error::CorruptRecord {
                            position: events.len() as u64,
                            detail: "mid-file corruption: valid batch follows \
                                     corrupt data"
                                .to_string(),
                        });
                    }
                    // Trailing corruption -- truncate.
                    tracing::warn!(
                        batch_start_offset,
                        valid_events = events.len(),
                        "truncating trailing corrupt data at byte offset \
                         {batch_start_offset}"
                    );
                    return truncate_and_return(path, batch_start_offset, events, streams);
                }
                Err(e) => return Err(e),
            };

            // Step 2: Decode record_count records.
            let mut batch_events = Vec::with_capacity(header.record_count as usize);
            for _ in 0..header.record_count {
                match codec::decode_record(&data[offset..]) {
                    Ok(DecodeOutcome::Complete { value, consumed }) => {
                        offset += consumed;
                        batch_events.push(value);
                    }
                    Ok(DecodeOutcome::Incomplete) | Err(Error::CorruptRecord { .. }) => {
                        // Incomplete or corrupt record within batch -- truncate
                        // the entire batch.
                        tracing::warn!(
                            batch_start_offset,
                            valid_events = events.len(),
                            "truncating partial batch (incomplete/corrupt record) \
                             at byte offset {batch_start_offset}"
                        );
                        return truncate_and_return(path, batch_start_offset, events, streams);
                    }
                    Err(e) => return Err(e),
                }
            }

            // Step 3: Decode batch footer.
            let footer = match codec::decode_batch_footer(&data[offset..]) {
                Ok(DecodeOutcome::Complete { value, consumed }) => {
                    offset += consumed;
                    value
                }
                Ok(DecodeOutcome::Incomplete) => {
                    // Missing/incomplete footer -- truncate the entire batch.
                    tracing::warn!(
                        batch_start_offset,
                        valid_events = events.len(),
                        "truncating partial batch (incomplete footer) at byte \
                         offset {batch_start_offset}"
                    );
                    return truncate_and_return(path, batch_start_offset, events, streams);
                }
                Err(Error::CorruptRecord { .. }) => {
                    // Wrong footer magic -- truncate the entire batch.
                    tracing::warn!(
                        batch_start_offset,
                        valid_events = events.len(),
                        "truncating partial batch (corrupt footer magic) at byte \
                         offset {batch_start_offset}"
                    );
                    return truncate_and_return(path, batch_start_offset, events, streams);
                }
                Err(e) => return Err(e),
            };

            // Step 4: Verify batch CRC over header + record bytes.
            let header_plus_records = &data[batch_start_offset..offset - codec::BATCH_FOOTER_SIZE];
            let computed_crc = crc32fast::hash(header_plus_records);
            if footer.batch_crc != computed_crc {
                tracing::warn!(
                    batch_start_offset,
                    valid_events = events.len(),
                    "truncating batch with CRC mismatch at byte offset \
                     {batch_start_offset}"
                );
                return truncate_and_return(path, batch_start_offset, events, streams);
            }

            // Step 5: Batch is valid -- commit events to the in-memory index.
            for event in batch_events {
                let global_pos = event.global_position;
                let stream_id = event.stream_id;
                streams.entry(stream_id).or_default().push(global_pos);
                events.push(event);
            }
        }

        // All records decoded successfully. Open file for future appends.
        let file = OpenOptions::new().read(true).write(true).open(path)?;

        Ok(Store {
            file,
            log: Arc::new(RwLock::new(EventLog { events, streams })),
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
        let log = self.log.read().expect("EventLog RwLock poisoned");
        let len = log.events.len() as u64;
        let start = from_position.min(len);
        let end = from_position.saturating_add(max_count).min(len);
        log.events[start as usize..end as usize].to_vec()
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
        // Step 1: Acquire a read lock to validate expected version and compute
        // starting positions. The read lock is held only for validation and
        // position computation, not during disk I/O.
        let (mut next_global, mut next_stream_version) = {
            let log = self.log.read().expect("EventLog RwLock poisoned");
            let stream_positions = log.streams.get(&stream_id);

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

            let next_global = log.events.len() as u64;
            let next_stream_version = stream_positions.map(|p| p.len() as u64).unwrap_or(0);
            (next_global, next_stream_version)
            // Read lock dropped here.
        };

        // Step 2: Build RecordedEvents and validate each one (no lock held).
        // The first_global_pos is captured before the loop increments next_global.
        let first_global_pos = next_global;
        let mut recorded = Vec::with_capacity(proposed_events.len());
        let mut encoded_records = Vec::new();

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

            encoded_records.extend_from_slice(&encoded);
            recorded.push(event);
            next_global += 1;
            next_stream_version += 1;
        }

        // Step 3: Wrap records in a batch envelope (header + records + footer).
        let batch_header = codec::encode_batch_header(recorded.len() as u32, first_global_pos);

        // CRC32 covers the header bytes concatenated with all record bytes.
        let batch_crc = {
            let mut hasher = crc32fast::Hasher::new();
            hasher.update(&batch_header);
            hasher.update(&encoded_records);
            hasher.finalize()
        };
        let batch_footer = codec::encode_batch_footer(batch_crc);

        // Build the contiguous buffer: header || records || footer.
        let mut encoded_batch = Vec::with_capacity(
            codec::BATCH_HEADER_SIZE + encoded_records.len() + codec::BATCH_FOOTER_SIZE,
        );
        encoded_batch.extend_from_slice(&batch_header);
        encoded_batch.extend_from_slice(&encoded_records);
        encoded_batch.extend_from_slice(&batch_footer);

        // Step 4: Write the entire batch envelope to disk and fsync (no lock held).
        use std::io::Seek;
        self.file.seek(std::io::SeekFrom::End(0))?;
        self.file.write_all(&encoded_batch)?;
        self.file.sync_all()?;

        // Step 5: Acquire write lock to update in-memory index (after fsync).
        {
            let mut log = self.log.write().expect("EventLog RwLock poisoned");
            let stream_entry = log.streams.entry(stream_id).or_default();
            for event in &recorded {
                stream_entry.push(event.global_position);
            }
            log.events.extend(recorded.clone());
        }

        Ok(recorded)
    }

    /// Returns the current byte length of the log file.
    ///
    /// Called after each successful append to update the `eventfold_log_bytes` gauge.
    /// Uses `File::metadata()` which issues a `stat(2)` syscall without seeking.
    ///
    /// # Returns
    ///
    /// The file size in bytes.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Io`] if the metadata syscall fails.
    pub fn log_file_len(&self) -> Result<u64, Error> {
        Ok(self.file.metadata()?.len())
    }

    /// Returns a clone of the shared `Arc<RwLock<EventLog>>`.
    ///
    /// This allows external components (such as `ReadIndex`) to hold a reference
    /// to the in-memory event log for concurrent read access while the writer task
    /// holds exclusive write access through the same `Arc`.
    ///
    /// # Returns
    ///
    /// A cloned `Arc<RwLock<EventLog>>` pointing to the same underlying event log.
    pub fn log(&self) -> Arc<RwLock<EventLog>> {
        Arc::clone(&self.log)
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

    /// Helper: write a seeded log file with the given events wrapped in a
    /// single batch envelope (header + batch_header + records + batch_footer).
    fn seed_file(path: &std::path::Path, events: &[RecordedEvent]) {
        use std::io::Write;
        let mut file = File::create(path).expect("create seed file");
        file.write_all(&codec::encode_header())
            .expect("write header");
        if !events.is_empty() {
            file.write_all(&encode_batch(events)).expect("write batch");
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
        let log = store.log.read().expect("read lock should not be poisoned");
        for (i, expected) in events.iter().enumerate() {
            let recovered = &log.events[i];
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

    // -- AC-4: Two batches; corrupt a record in the second batch. With batch
    // recovery, the entire second batch is discarded. Only the first batch's
    // events are recovered.
    #[test]
    fn recovery_truncates_corrupt_record_in_last_batch() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        let stream = Uuid::new_v4();

        // Batch 1: 2 events.
        let batch1 = vec![
            make_event(0, stream, 0, "Evt", b"payload0"),
            make_event(1, stream, 1, "Evt", b"payload1"),
        ];
        // Batch 2: 1 event.
        let batch2 = vec![make_event(2, stream, 2, "Evt", b"payload2")];

        seed_batch_file(&path, &[&batch1, &batch2]);

        // Compute the byte offset of the second batch's start.
        let batch1_bytes = encode_batch(&batch1);
        let valid_boundary = HEADER_SIZE + batch1_bytes.len();

        // Read the file, flip a byte in the second batch's record body
        // (inside the payload, before the per-record CRC).
        let mut data = std::fs::read(&path).expect("read file");
        // The last 4 bytes of the file are batch_footer CRC. Before that is the
        // per-record CRC (4 bytes) at the end of the record. Flip a byte in the
        // record's payload region: 13 bytes before the file end is well within
        // the record body.
        let corrupt_idx = data.len() - 13;
        data[corrupt_idx] ^= 0xFF;
        std::fs::write(&path, &data).expect("write corrupted file");

        // Reopen -- should recover only the first 2 events (batch 1).
        let store = Store::open(&path).expect("recovery should succeed");

        assert_eq!(store.global_position(), 2);
        assert_eq!(store.stream_version(&stream), Some(1));

        // File should be truncated to the boundary before the 2nd batch.
        let final_size = std::fs::metadata(&path).expect("metadata").len() as usize;
        assert_eq!(
            final_size, valid_boundary,
            "file should be truncated to end of batch 1"
        );
    }

    // -- AC-5: Corrupt the batch header of the first batch (so it's
    // unrecognized), but a valid second batch follows. This is mid-file
    // corruption because valid data follows the corrupt region.
    #[test]
    fn recovery_returns_error_on_mid_file_corruption() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        let stream = Uuid::new_v4();

        // Batch 1: 1 event.
        let batch1 = vec![make_event(0, stream, 0, "Evt", b"payload0")];
        // Batch 2: 1 event.
        let batch2 = vec![make_event(1, stream, 1, "Evt", b"payload1")];

        seed_batch_file(&path, &[&batch1, &batch2]);

        // Corrupt the batch header magic of batch 1 (at offset HEADER_SIZE).
        let mut data = std::fs::read(&path).expect("read file");
        data[HEADER_SIZE] ^= 0xFF; // corrupt first byte of batch 1 header magic
        std::fs::write(&path, &data).expect("write corrupted file");

        // Reopen -- should return CorruptRecord because a valid batch 2 follows.
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

    // -- Ticket 2 AC: Store::open on new path -> store.log() returns Arc with empty EventLog.
    #[test]
    fn log_accessor_returns_empty_event_log_on_new_store() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        let store = Store::open(&path).expect("open should succeed");
        let log = store.log();
        let guard = log.read().expect("read lock should not be poisoned");

        assert_eq!(guard.events.len(), 0);
        assert_eq!(guard.streams.len(), 0);
    }

    // -- Ticket 2 AC: After appending one event, log() read-locked EventLog reflects it.
    #[test]
    fn log_accessor_reflects_appended_event() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        let mut store = Store::open(&path).expect("open should succeed");
        let stream_id = Uuid::new_v4();
        let proposed = vec![make_proposed("Created", b"{}")];
        store
            .append(stream_id, ExpectedVersion::NoStream, proposed)
            .expect("append should succeed");

        let log = store.log();
        let guard = log.read().expect("read lock should not be poisoned");

        assert_eq!(guard.events.len(), 1);
        assert!(
            guard.streams.contains_key(&stream_id),
            "streams should contain the appended stream's UUID"
        );
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

    // -- PRD 008 Ticket 2: Batch envelope tests --

    // AC-1: Append 3 events to a fresh store. Read raw bytes.
    // After the 8-byte file header: bytes 0..16 decode as a valid BatchHeader
    // with record_count==3 and first_global_pos==0. Next bytes are 3
    // individually-decodable records. Next 8 bytes decode as a valid
    // BatchFooter with the correct CRC32. No extra bytes remain.
    #[test]
    fn batch_envelope_raw_bytes_three_events() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        let mut store = Store::open(&path).expect("open should succeed");
        let stream_id = Uuid::new_v4();
        let proposed = vec![
            make_proposed("Evt0", b"p0"),
            make_proposed("Evt1", b"p1"),
            make_proposed("Evt2", b"p2"),
        ];

        store
            .append(stream_id, ExpectedVersion::NoStream, proposed)
            .expect("append should succeed");

        // Read raw file bytes (do NOT drop and reopen the store).
        let data = std::fs::read(&path).expect("read file");

        // Skip the 8-byte file header.
        let batch_data = &data[HEADER_SIZE..];

        // First 16 bytes: batch header.
        let header_result = codec::decode_batch_header(&batch_data[..codec::BATCH_HEADER_SIZE])
            .expect("decode batch header should not error");
        let (header, header_consumed) = match header_result {
            DecodeOutcome::Complete { value, consumed } => (value, consumed),
            DecodeOutcome::Incomplete => panic!("batch header should be complete"),
        };
        assert_eq!(header.record_count, 3);
        assert_eq!(header.first_global_pos, 0);
        assert_eq!(header_consumed, codec::BATCH_HEADER_SIZE);

        // Next: 3 individually-decodable records.
        let mut offset = codec::BATCH_HEADER_SIZE;
        for i in 0u64..3 {
            let record_result = codec::decode_record(&batch_data[offset..])
                .unwrap_or_else(|e| panic!("decode record {i} should succeed: {e}"));
            match record_result {
                DecodeOutcome::Complete { value, consumed } => {
                    assert_eq!(value.global_position, i, "record {i} global_position");
                    offset += consumed;
                }
                DecodeOutcome::Incomplete => panic!("record {i} should be complete"),
            }
        }

        // Next 8 bytes: batch footer.
        let footer_result = codec::decode_batch_footer(&batch_data[offset..])
            .expect("decode batch footer should not error");
        let (footer, footer_consumed) = match footer_result {
            DecodeOutcome::Complete { value, consumed } => (value, consumed),
            DecodeOutcome::Incomplete => panic!("batch footer should be complete"),
        };
        assert_eq!(footer_consumed, codec::BATCH_FOOTER_SIZE);

        // Verify CRC32: recompute over header bytes + record bytes.
        let header_plus_records = &batch_data[..offset];
        let expected_crc = crc32fast::hash(header_plus_records);
        assert_eq!(
            footer.batch_crc, expected_crc,
            "batch footer CRC should match recomputed value"
        );

        // No extra bytes remain after the footer.
        let total_consumed = offset + footer_consumed;
        assert_eq!(
            total_consumed,
            batch_data.len(),
            "no extra bytes should remain after batch footer"
        );
    }

    // AC-2: Append two separate batches (2 events, then 1 event). Read raw
    // file bytes. Verify two consecutive batch envelopes: first has
    // record_count==2, second has record_count==1 and first_global_pos==2.
    #[test]
    fn batch_envelope_two_consecutive_batches() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        let mut store = Store::open(&path).expect("open should succeed");
        let stream_id = Uuid::new_v4();

        // First batch: 2 events.
        let batch1 = vec![make_proposed("Evt0", b"p0"), make_proposed("Evt1", b"p1")];
        store
            .append(stream_id, ExpectedVersion::NoStream, batch1)
            .expect("first append should succeed");

        // Second batch: 1 event.
        let batch2 = vec![make_proposed("Evt2", b"p2")];
        store
            .append(stream_id, ExpectedVersion::Exact(1), batch2)
            .expect("second append should succeed");

        let data = std::fs::read(&path).expect("read file");
        let batch_data = &data[HEADER_SIZE..];

        // --- First batch envelope ---
        let h1 = match codec::decode_batch_header(batch_data).expect("decode batch header 1") {
            DecodeOutcome::Complete { value, .. } => value,
            DecodeOutcome::Incomplete => panic!("batch header 1 should be complete"),
        };
        assert_eq!(h1.record_count, 2);
        assert_eq!(h1.first_global_pos, 0);

        // Decode 2 records.
        let mut offset = codec::BATCH_HEADER_SIZE;
        for i in 0u64..2 {
            match codec::decode_record(&batch_data[offset..])
                .unwrap_or_else(|e| panic!("decode record {i}: {e}"))
            {
                DecodeOutcome::Complete { consumed, .. } => offset += consumed,
                DecodeOutcome::Incomplete => panic!("record {i} should be complete"),
            }
        }

        // Decode footer 1 and verify CRC.
        let f1 = match codec::decode_batch_footer(&batch_data[offset..])
            .expect("decode batch footer 1")
        {
            DecodeOutcome::Complete { value, .. } => value,
            DecodeOutcome::Incomplete => panic!("batch footer 1 should be complete"),
        };
        let expected_crc1 = crc32fast::hash(&batch_data[..offset]);
        assert_eq!(f1.batch_crc, expected_crc1, "batch 1 CRC mismatch");
        offset += codec::BATCH_FOOTER_SIZE;

        // --- Second batch envelope ---
        let batch2_start = offset;
        let h2 = match codec::decode_batch_header(&batch_data[offset..])
            .expect("decode batch header 2")
        {
            DecodeOutcome::Complete { value, .. } => value,
            DecodeOutcome::Incomplete => panic!("batch header 2 should be complete"),
        };
        assert_eq!(h2.record_count, 1);
        assert_eq!(h2.first_global_pos, 2);

        offset += codec::BATCH_HEADER_SIZE;

        // Decode 1 record.
        match codec::decode_record(&batch_data[offset..]).expect("decode record 2") {
            DecodeOutcome::Complete { consumed, .. } => offset += consumed,
            DecodeOutcome::Incomplete => panic!("record 2 should be complete"),
        }

        // Decode footer 2 and verify CRC.
        let f2 = match codec::decode_batch_footer(&batch_data[offset..])
            .expect("decode batch footer 2")
        {
            DecodeOutcome::Complete { value, .. } => value,
            DecodeOutcome::Incomplete => panic!("batch footer 2 should be complete"),
        };
        let expected_crc2 = crc32fast::hash(&batch_data[batch2_start..offset]);
        assert_eq!(f2.batch_crc, expected_crc2, "batch 2 CRC mismatch");
        offset += codec::BATCH_FOOTER_SIZE;

        // No extra bytes.
        assert_eq!(
            offset,
            batch_data.len(),
            "no extra bytes after second batch"
        );
    }

    // AC-3: Store::append still returns the correct Vec<RecordedEvent> with
    // correct global_position and stream_version values after the batch
    // envelope change.
    #[test]
    fn batch_envelope_append_returns_correct_recorded_events() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        let mut store = Store::open(&path).expect("open should succeed");
        let stream_id = Uuid::new_v4();

        // First batch: 2 events to a new stream.
        let batch1 = vec![
            make_proposed("OrderPlaced", b"{\"qty\":1}"),
            make_proposed("OrderConfirmed", b"{\"status\":\"ok\"}"),
        ];
        let recorded1 = store
            .append(stream_id, ExpectedVersion::NoStream, batch1)
            .expect("first append should succeed");

        assert_eq!(recorded1.len(), 2);
        assert_eq!(recorded1[0].global_position, 0);
        assert_eq!(recorded1[0].stream_version, 0);
        assert_eq!(recorded1[0].stream_id, stream_id);
        assert_eq!(recorded1[0].event_type, "OrderPlaced");
        assert_eq!(recorded1[1].global_position, 1);
        assert_eq!(recorded1[1].stream_version, 1);
        assert_eq!(recorded1[1].stream_id, stream_id);
        assert_eq!(recorded1[1].event_type, "OrderConfirmed");

        // Second batch: 1 event to the same stream.
        let batch2 = vec![make_proposed("OrderShipped", b"{\"carrier\":\"ups\"}")];
        let recorded2 = store
            .append(stream_id, ExpectedVersion::Exact(1), batch2)
            .expect("second append should succeed");

        assert_eq!(recorded2.len(), 1);
        assert_eq!(recorded2[0].global_position, 2);
        assert_eq!(recorded2[0].stream_version, 2);
        assert_eq!(recorded2[0].stream_id, stream_id);
        assert_eq!(recorded2[0].event_type, "OrderShipped");

        // Verify store-level state.
        assert_eq!(store.global_position(), 3);
        assert_eq!(store.stream_version(&stream_id), Some(2));
    }

    // -- PRD 008 Ticket 3: Batch-aware recovery tests --

    /// Helper: write a complete batch (header + records + footer) to a buffer.
    ///
    /// Returns the encoded bytes for one batch envelope containing the given
    /// events. The CRC covers the header + all record bytes.
    fn encode_batch(events: &[RecordedEvent]) -> Vec<u8> {
        let header_bytes =
            codec::encode_batch_header(events.len() as u32, events[0].global_position);
        let mut record_bytes = Vec::new();
        for event in events {
            record_bytes.extend_from_slice(&codec::encode_record(event));
        }
        let batch_crc = {
            let mut hasher = crc32fast::Hasher::new();
            hasher.update(&header_bytes);
            hasher.update(&record_bytes);
            hasher.finalize()
        };
        let footer_bytes = codec::encode_batch_footer(batch_crc);

        let mut buf = Vec::with_capacity(
            codec::BATCH_HEADER_SIZE + record_bytes.len() + codec::BATCH_FOOTER_SIZE,
        );
        buf.extend_from_slice(&header_bytes);
        buf.extend_from_slice(&record_bytes);
        buf.extend_from_slice(&footer_bytes);
        buf
    }

    /// Helper: write a log file with the file header followed by the given
    /// batch-formatted event groups.
    fn seed_batch_file(path: &std::path::Path, batches: &[&[RecordedEvent]]) {
        use std::io::Write;
        let mut file = File::create(path).expect("create seed file");
        file.write_all(&codec::encode_header())
            .expect("write header");
        for batch in batches {
            file.write_all(&encode_batch(batch)).expect("write batch");
        }
        file.sync_all().expect("sync seed file");
    }

    // AC-6: Store::open on a non-existent path returns 0 events. A second
    // Store::open on the same path also returns 0 events (validating the
    // directory fsync ensures the file is findable after simulated restart).
    #[test]
    fn open_new_file_dir_fsync_and_reopen() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        assert!(!path.exists());

        // First open: creates the file.
        let store = Store::open(&path).expect("first open should succeed");
        assert_eq!(store.global_position(), 0);
        assert!(store.read_all(0, 100).is_empty());
        drop(store);

        // Second open: should find the file (dir entry was fsynced).
        let store2 = Store::open(&path).expect("second open should succeed");
        assert_eq!(store2.global_position(), 0);
        assert!(store2.read_all(0, 100).is_empty());
    }

    // AC-2: Write a complete batch (header + 2 records + footer), then
    // remove the last 8 bytes (footer). Store::open should return 0 events
    // and truncate the file to HEADER_SIZE (the start of the incomplete batch).
    #[test]
    fn recovery_truncates_batch_missing_footer() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        let stream = Uuid::new_v4();
        let events = vec![
            make_event(0, stream, 0, "Evt", b"p0"),
            make_event(1, stream, 1, "Evt", b"p1"),
        ];

        // Write file header + complete batch.
        seed_batch_file(&path, &[&events]);

        // Remove the last 8 bytes (the footer).
        let original_size = std::fs::metadata(&path).expect("metadata").len();
        let file = OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("open for truncation");
        file.set_len(original_size - codec::BATCH_FOOTER_SIZE as u64)
            .expect("truncate footer");
        file.sync_all().expect("sync");

        let store = Store::open(&path).expect("recovery should succeed");

        // No events recovered (the only batch was incomplete).
        assert_eq!(store.global_position(), 0);
        assert!(store.read_all(0, 100).is_empty());

        // File should be truncated to just the file header.
        let final_size = std::fs::metadata(&path).expect("metadata").len();
        assert_eq!(
            final_size, HEADER_SIZE as u64,
            "file should be truncated to file header (batch start offset)"
        );
    }

    // AC-3: Write a batch header + 2 records, then truncate after the first
    // record's first 4 bytes (mid-record). No footer. Store::open should
    // return 0 events and truncate the file to HEADER_SIZE.
    #[test]
    fn recovery_truncates_batch_mid_record_truncation() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        let stream = Uuid::new_v4();
        let events = [
            make_event(0, stream, 0, "Evt", b"p0"),
            make_event(1, stream, 1, "Evt", b"p1"),
        ];

        // Manually construct: file header + batch header + first record
        // + first 4 bytes of second record.
        use std::io::Write;
        let mut file = File::create(&path).expect("create file");
        file.write_all(&codec::encode_header())
            .expect("write file header");

        let batch_header = codec::encode_batch_header(2, 0);
        file.write_all(&batch_header).expect("write batch header");

        let rec0 = codec::encode_record(&events[0]);
        file.write_all(&rec0).expect("write record 0");

        let rec1 = codec::encode_record(&events[1]);
        // Write only the first 4 bytes of record 1 (mid-record truncation).
        file.write_all(&rec1[..4]).expect("write partial record 1");
        file.sync_all().expect("sync");

        let store = Store::open(&path).expect("recovery should succeed");

        assert_eq!(store.global_position(), 0);
        assert!(store.read_all(0, 100).is_empty());

        let final_size = std::fs::metadata(&path).expect("metadata").len();
        assert_eq!(
            final_size, HEADER_SIZE as u64,
            "file should be truncated to file header (batch start offset)"
        );
    }

    // AC-4: Two complete batches followed by an incomplete third batch
    // (header + 1 of 2 records, no footer). Store::open should recover
    // exactly the events from the first two batches.
    #[test]
    fn recovery_two_complete_batches_plus_partial_third() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        let stream = Uuid::new_v4();

        // Batch 1: 2 events (global positions 0, 1).
        let batch1_events = vec![
            make_event(0, stream, 0, "Evt", b"p0"),
            make_event(1, stream, 1, "Evt", b"p1"),
        ];
        // Batch 2: 1 event (global position 2).
        let batch2_events = vec![make_event(2, stream, 2, "Evt", b"p2")];
        // Batch 3 (incomplete): header says 2 records, but only 1 written, no footer.
        let batch3_event = make_event(3, stream, 3, "Evt", b"p3");

        use std::io::Write;
        let mut file = File::create(&path).expect("create file");
        file.write_all(&codec::encode_header())
            .expect("write file header");

        // Write two complete batches.
        file.write_all(&encode_batch(&batch1_events))
            .expect("write batch 1");
        file.write_all(&encode_batch(&batch2_events))
            .expect("write batch 2");

        // Write incomplete third batch: header (says 2 records) + 1 record, no footer.
        let batch3_header = codec::encode_batch_header(2, 3);
        file.write_all(&batch3_header)
            .expect("write batch 3 header");
        file.write_all(&codec::encode_record(&batch3_event))
            .expect("write batch 3 record 0");
        // Omit second record and footer.
        file.sync_all().expect("sync");

        let store = Store::open(&path).expect("recovery should succeed");

        // Should recover exactly 3 events from the first two batches.
        assert_eq!(store.global_position(), 3);

        let all = store.read_all(0, 100);
        assert_eq!(all.len(), 3);
        for (i, event) in all.iter().enumerate() {
            assert_eq!(event.global_position, i as u64);
        }
    }

    // AC-7: Write two complete batches manually. Store::open recovers all
    // events in global-position order with no gaps.
    #[test]
    fn recovery_two_complete_batches_all_events_correct() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        let stream_a = Uuid::new_v4();
        let stream_b = Uuid::new_v4();

        // Batch 1: 2 events on stream_a.
        let batch1 = vec![
            make_event(0, stream_a, 0, "A0", b"a0"),
            make_event(1, stream_a, 1, "A1", b"a1"),
        ];
        // Batch 2: 2 events on stream_b.
        let batch2 = vec![
            make_event(2, stream_b, 0, "B0", b"b0"),
            make_event(3, stream_b, 1, "B1", b"b1"),
        ];

        seed_batch_file(&path, &[&batch1, &batch2]);

        let store = Store::open(&path).expect("recovery should succeed");

        assert_eq!(store.global_position(), 4);

        let all = store.read_all(0, 100);
        assert_eq!(all.len(), 4);
        for (i, event) in all.iter().enumerate() {
            assert_eq!(
                event.global_position, i as u64,
                "global_position at index {i}"
            );
        }
        // Verify event types to ensure correct ordering.
        assert_eq!(all[0].event_type, "A0");
        assert_eq!(all[1].event_type, "A1");
        assert_eq!(all[2].event_type, "B0");
        assert_eq!(all[3].event_type, "B1");

        // Verify stream index integrity.
        assert_eq!(store.stream_version(&stream_a), Some(1));
        assert_eq!(store.stream_version(&stream_b), Some(1));
    }

    // AC-5: File with FORMAT_VERSION=1 header -> Err(InvalidHeader) mentioning "version".
    #[test]
    fn recovery_rejects_version_1_file() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        // Write old FORMAT_VERSION=1 file header: magic EFDB + version 1 LE.
        let mut header = [0u8; 8];
        header[0..4].copy_from_slice(&[0x45, 0x46, 0x44, 0x42]); // EFDB
        header[4..8].copy_from_slice(&1u32.to_le_bytes()); // version 1
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

    // AC-4: store.read_all(0, 100) after a 3-event append returns all 3
    // events in order.
    #[test]
    fn batch_envelope_read_all_after_three_event_append() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");

        let mut store = Store::open(&path).expect("open should succeed");
        let stream_id = Uuid::new_v4();
        let proposed = vec![
            make_proposed("Evt0", b"p0"),
            make_proposed("Evt1", b"p1"),
            make_proposed("Evt2", b"p2"),
        ];

        store
            .append(stream_id, ExpectedVersion::NoStream, proposed)
            .expect("append should succeed");

        let events = store.read_all(0, 100);
        assert_eq!(events.len(), 3);
        for (i, event) in events.iter().enumerate() {
            assert_eq!(
                event.global_position, i as u64,
                "global_position for event {i}"
            );
            assert_eq!(
                event.stream_version, i as u64,
                "stream_version for event {i}"
            );
            assert_eq!(event.stream_id, stream_id);
        }
    }

    // --- log_file_len tests (PRD 013, Ticket 2) ---

    #[test]
    fn log_file_len_fresh_store_returns_at_least_header_size() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");
        let store = Store::open(&path).expect("open should succeed");

        let len = store.log_file_len().expect("log_file_len should succeed");
        assert!(
            len >= 8,
            "fresh store log file length should be >= 8 (header size), got {len}"
        );
    }

    #[test]
    fn log_file_len_grows_after_append() {
        let dir = tempfile::tempdir().expect("failed to create tempdir");
        let path = dir.path().join("events.log");
        let mut store = Store::open(&path).expect("open should succeed");

        let before = store.log_file_len().expect("log_file_len before append");

        let proposed = ProposedEvent {
            event_id: Uuid::new_v4(),
            event_type: "TestEvent".to_string(),
            metadata: Bytes::new(),
            payload: Bytes::from_static(b"{}"),
        };
        store
            .append(Uuid::new_v4(), ExpectedVersion::Any, vec![proposed])
            .expect("append should succeed");

        let after = store.log_file_len().expect("log_file_len after append");
        assert!(
            after > before,
            "log file should grow after append: before={before}, after={after}"
        );
    }
}
