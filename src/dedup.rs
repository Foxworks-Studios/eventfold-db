//! Bounded LRU deduplication index for idempotent appends.
//!
//! Tracks recently written event IDs so that retried appends can be detected
//! and the original recorded events returned instead of writing duplicates.
//! The index uses an LRU cache keyed by event ID, so the most recently written
//! events remain dedup-eligible while older entries are evicted.

use std::num::NonZeroUsize;
use std::sync::Arc;

use lru::LruCache;
use uuid::Uuid;

use crate::types::{ProposedEvent, RecordedEvent};

/// Bounded LRU index mapping event IDs to their recorded batch.
///
/// Each entry maps one event ID (`Uuid`) to the full batch of `RecordedEvent`s
/// that were written together. Multiple event IDs from the same batch share a
/// single `Arc<Vec<RecordedEvent>>` allocation.
///
/// The dedup check uses only the first event ID in a proposed batch as the
/// lookup key, because appends are atomic at the batch level -- if the first
/// event is a known duplicate, the entire batch is.
pub struct DedupIndex {
    /// LRU cache mapping event IDs to the batch of recorded events that
    /// contained them. Multiple keys from the same batch point to the same
    /// `Arc` allocation.
    cache: LruCache<Uuid, Arc<Vec<RecordedEvent>>>,
}

impl DedupIndex {
    /// Create a new dedup index with the given capacity (number of event IDs tracked).
    ///
    /// # Arguments
    ///
    /// * `capacity` - Maximum number of event ID entries before LRU eviction kicks in.
    pub fn new(capacity: NonZeroUsize) -> Self {
        Self {
            cache: LruCache::new(capacity),
        }
    }

    /// Check whether a proposed batch is a duplicate of a previously recorded batch.
    ///
    /// Looks up the `event_id` of the first event in `proposed`. If found in the
    /// cache, returns the original recorded batch as `Some(Arc<Vec<RecordedEvent>>)`.
    /// Returns `None` for an empty slice or if the first event ID is not cached.
    ///
    /// # Arguments
    ///
    /// * `proposed` - The batch of proposed events to check for duplicates.
    ///
    /// # Returns
    ///
    /// `Some(recorded_batch)` if the first event's ID was previously recorded,
    /// `None` otherwise (including when `proposed` is empty).
    pub fn check(&mut self, proposed: &[ProposedEvent]) -> Option<Arc<Vec<RecordedEvent>>> {
        let first = proposed.first()?;
        // get() promotes the entry in LRU order, keeping retried batches warm.
        self.cache.get(&first.event_id).cloned()
    }

    /// Record a successfully written batch in the dedup cache.
    ///
    /// Creates a single `Arc<Vec<RecordedEvent>>` for the batch and inserts one
    /// cache entry per event ID in `recorded`, all pointing to the same `Arc`.
    /// This means looking up any event ID from the batch will return the full
    /// batch of recorded events.
    ///
    /// # Arguments
    ///
    /// * `recorded` - The batch of recorded events to cache for future dedup checks.
    pub fn record(&mut self, recorded: Vec<RecordedEvent>) {
        let shared = Arc::new(recorded);
        for event in shared.iter() {
            self.cache.put(event.event_id, Arc::clone(&shared));
        }
    }

    /// Seed the dedup index from recovered events during startup.
    ///
    /// Inserts each event individually (as a single-event batch) in ascending
    /// global-position order. Because `LruCache::put` marks the inserted key as
    /// most-recently-used, inserting oldest-first means the highest-position
    /// events end up LRU-hottest on completion.
    ///
    /// If the total number of events exceeds the cache capacity, the oldest
    /// (lowest global position) events are naturally evicted by the LRU policy.
    ///
    /// # Arguments
    ///
    /// * `events` - Slice of recorded events in ascending global-position order.
    pub fn seed_from_log(&mut self, events: &[RecordedEvent]) {
        for event in events {
            // Each event becomes its own single-element batch when seeding.
            // This matches the dedup semantics: each event ID maps to a batch
            // containing that event's data.
            let batch = Arc::new(vec![event.clone()]);
            self.cache.put(event.event_id, batch);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use bytes::Bytes;

    /// Helper to create a `ProposedEvent` with a given event ID.
    fn proposed(event_id: Uuid) -> ProposedEvent {
        ProposedEvent {
            event_id,
            event_type: "TestEvent".to_string(),
            metadata: Bytes::new(),
            payload: Bytes::from_static(b"{}"),
        }
    }

    /// Helper to create a `RecordedEvent` with given IDs and positions.
    fn recorded(event_id: Uuid, stream_id: Uuid, version: u64, position: u64) -> RecordedEvent {
        RecordedEvent {
            event_id,
            stream_id,
            stream_version: version,
            global_position: position,
            recorded_at: 0,
            event_type: "TestEvent".to_string(),
            metadata: Bytes::new(),
            payload: Bytes::from_static(b"{}"),
        }
    }

    #[test]
    fn check_empty_slice_returns_none() {
        let mut index = DedupIndex::new(NonZeroUsize::new(4).expect("nonzero"));
        assert!(index.check(&[]).is_none());
    }

    #[test]
    fn record_batch_then_check_returns_same_arc() {
        let mut index = DedupIndex::new(NonZeroUsize::new(4).expect("nonzero"));
        let stream = Uuid::new_v4();
        let id_a = Uuid::new_v4();
        let id_b = Uuid::new_v4();

        let batch = vec![recorded(id_a, stream, 0, 0), recorded(id_b, stream, 1, 1)];
        index.record(batch);

        // Check with a proposed batch whose first event has ID A
        let result_a = index.check(&[proposed(id_a)]);
        assert!(result_a.is_some());
        let arc_a = result_a.expect("should be Some");
        assert_eq!(arc_a.len(), 2);

        // Check with a proposed batch whose first event has ID B
        let result_b = index.check(&[proposed(id_b)]);
        assert!(result_b.is_some());
        let arc_b = result_b.expect("should be Some");

        // Both must point to the same Arc allocation
        assert!(Arc::ptr_eq(&arc_a, &arc_b));
    }

    #[test]
    fn check_unknown_event_id_returns_none() {
        let mut index = DedupIndex::new(NonZeroUsize::new(4).expect("nonzero"));
        let stream = Uuid::new_v4();
        let known_id = Uuid::new_v4();

        index.record(vec![recorded(known_id, stream, 0, 0)]);

        // A proposed batch with an unknown first event ID should miss the cache
        let unknown_id = Uuid::new_v4();
        assert!(index.check(&[proposed(unknown_id)]).is_none());
    }

    #[test]
    fn lru_eviction_drops_oldest_entry() {
        // Capacity of 2 event IDs
        let mut index = DedupIndex::new(NonZeroUsize::new(2).expect("nonzero"));
        let stream = Uuid::new_v4();

        let id_x = Uuid::new_v4();
        let id_y = Uuid::new_v4();
        let id_z = Uuid::new_v4();

        // Fill to capacity with two single-event batches
        index.record(vec![recorded(id_x, stream, 0, 0)]);
        index.record(vec![recorded(id_y, stream, 1, 1)]);

        // Both should be present
        assert!(index.check(&[proposed(id_x)]).is_some());
        assert!(index.check(&[proposed(id_y)]).is_some());

        // Record a third batch -- this should evict X (LRU)
        index.record(vec![recorded(id_z, stream, 2, 2)]);

        // X was evicted (least recently used), Y and Z remain
        assert!(index.check(&[proposed(id_x)]).is_none());
        assert!(index.check(&[proposed(id_y)]).is_some());
        assert!(index.check(&[proposed(id_z)]).is_some());
    }

    #[test]
    fn seed_from_log_evicts_oldest_positions() {
        // Capacity 3, seed with 5 events (positions 0..4).
        // Only positions 2, 3, 4 should remain.
        let mut index = DedupIndex::new(NonZeroUsize::new(3).expect("nonzero"));
        let stream = Uuid::new_v4();

        let ids: Vec<Uuid> = (0..5).map(|_| Uuid::new_v4()).collect();
        let events: Vec<RecordedEvent> = ids
            .iter()
            .enumerate()
            .map(|(i, &id)| recorded(id, stream, i as u64, i as u64))
            .collect();

        index.seed_from_log(&events);

        // Positions 0, 1 should have been evicted
        assert!(index.check(&[proposed(ids[0])]).is_none());
        assert!(index.check(&[proposed(ids[1])]).is_none());

        // Positions 2, 3, 4 should remain
        assert!(index.check(&[proposed(ids[2])]).is_some());
        assert!(index.check(&[proposed(ids[3])]).is_some());
        assert!(index.check(&[proposed(ids[4])]).is_some());
    }

    #[test]
    fn seed_from_log_check_returns_correct_event_data() {
        let mut index = DedupIndex::new(NonZeroUsize::new(8).expect("nonzero"));
        let stream = Uuid::new_v4();
        let id = Uuid::new_v4();

        let events = vec![recorded(id, stream, 3, 7)];
        index.seed_from_log(&events);

        let result = index
            .check(&[proposed(id)])
            .expect("seeded event should be found");
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].event_id, id);
        assert_eq!(result[0].stream_id, stream);
        assert_eq!(result[0].global_position, 7);
        assert_eq!(result[0].stream_version, 3);
    }
}
