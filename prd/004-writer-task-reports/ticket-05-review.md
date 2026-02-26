# Code Review: Ticket 5 -- `run_writer` Task Loop and `spawn_writer`

**Ticket:** 5 -- `run_writer` Task Loop and `spawn_writer`
**Impl Report:** prd/004-writer-task-reports/ticket-05-impl.md
**Date:** 2026-02-25 15:00
**Verdict:** APPROVED

---

## AC Coverage

| AC # | Description | Status | Notes |
|------|-------------|--------|-------|
| `run_writer` sig | `pub(crate) async fn`, loops on `rx.recv()`, drains `try_recv()`, processes via `store.append()`, `tracing::warn!` on dropped receiver, clean exit on `None` | Met | `writer.rs:125-152` exactly matches the spec. `while let Some(first) = rx.recv().await` for blocking receive; inner `while let Ok(req) = rx.try_recv()` for draining; `if req.response_tx.send(result).is_err()` with `tracing::warn!` for dropped receiver; loop exits cleanly on `None`. |
| `spawn_writer` sig | `pub fn`, calls `store.log()` BEFORE moving `store`, constructs `ReadIndex`, returns `(WriterHandle, ReadIndex, JoinHandle<()>)` | Met | `writer.rs:171-189`. `store.log()` called on line 180, `ReadIndex::new(log_arc)` on line 181, `store` moved on line 186. Critical ordering is correct. |
| AC-1 | Basic append: `global_position == 0`, `stream_version == 0` | Met | `ac1_basic_append_through_writer` -- asserts `events[0].global_position == 0` and `events[0].stream_version == 0`. |
| AC-2 | 3 sequential appends with contiguous positions 0, 1, 2 and stream versions 0, 1, 2 | Met | `ac2_sequential_appends_have_contiguous_positions` -- awaits each append, checks `global_position` and `stream_version` on each returned slice. |
| AC-3 | 10 concurrent appends with `Any` -- unique positions forming `{0..9}` | Met | `ac3_concurrent_appends_serialized` -- spawns 10 tasks via `tokio::spawn`, collects positions into `HashSet`, asserts equality with `(0..10).collect()`. |
| AC-4a | `NoStream` twice returns `WrongExpectedVersion` | Met | `ac4a_nostream_twice_returns_wrong_expected_version` -- second append matches `Err(Error::WrongExpectedVersion { .. })`. |
| AC-4b | `NoStream` then `Exact(0)` succeeds | Met | `ac4b_exact_0_after_nostream_succeeds` -- asserts `result.is_ok()`. |
| AC-4c | `NoStream` then `Exact(5)` returns `WrongExpectedVersion` | Met | `ac4c_exact_5_after_nostream_returns_wrong_expected_version` -- matches `Err(Error::WrongExpectedVersion { .. })`. |
| AC-5 | `ReadIndex::read_all` and `ReadIndex::read_stream` reflect writes | Met | `ac5_read_index_reflects_writes` -- appends 3 events, calls `read_index.read_all(0, 100)` (asserts len==3) and `read_index.read_stream(stream_id, 0, 100)` (asserts len==3, correct versions). |
| AC-6 | Durability: 5 events survive restart | Met | `ac6_durability_survives_restart` -- first scope appends 5, drops handle, awaits join; second scope reopens store, constructs `ReadIndex`, asserts `read_all.len() == 5`. |
| AC-7 | Graceful shutdown: join resolves within 1 second | Met | `ac7_graceful_shutdown_on_handle_drop` -- drops `handle`, calls `tokio::time::timeout(Duration::from_secs(1), join_handle)`, asserts `result.is_ok()`. |
| AC-8 | Backpressure: `try_send` returns `Full` on capacity=1 | Met | `ac8_backpressure_bounded_channel` -- `try_send` to fill slot, second `try_send` matches `TrySendError::Full(_)`. See note below. |
| AC-9a | `EventTooLarge` for oversized payload | Met | `ac9a_event_too_large_returns_error` -- payload is `MAX_EVENT_SIZE + 1` bytes, asserts `Err(Error::EventTooLarge { .. })`. |
| AC-9b | Valid append succeeds after `EventTooLarge` (writer not poisoned) | Met | `ac9b_writer_not_poisoned_after_error` -- bad event fails, then a valid append returns `Ok`. |
| lib.rs re-export | `spawn_writer` added to `pub use writer::` line | Met | `lib.rs:17` -- `pub use writer::{WriterHandle, spawn_writer};`. |

---

## Issues Found

### Critical (must fix before merge)
None.

### Major (should fix, risk of downstream problems)
None.

### Minor (nice to fix, not blocking)

1. **AC-8 test bypasses `WriterHandle` public API to access private field `tx`.** `ac8_backpressure_bounded_channel` accesses `handle.tx.try_send(...)` directly. Because the test lives inside the same `writer.rs` module, this compiles (Rust allows same-module private access). However, this couples the test to the internal representation of `WriterHandle`. If `tx` is ever renamed or made into an `enum`, the test breaks without a compiler error at the API boundary. A cleaner approach would be to expose `try_append` on `WriterHandle` or to use `tokio::time::timeout` as the ticket's implementer note suggests. Not blocking -- behavior is correct and the access pattern is valid Rust.

2. **Batch drain loop processes all requests sequentially per batch before replying to any caller.** In `run_writer`, the drain loop collects the entire batch into a `Vec`, then the processing loop calls `store.append()` (which includes fsync) for each item before sending any response. This means callers at the back of a batch wait for all preceding fsyncs before their response is sent. For the current single-node use case this is acceptable and matches the described design, but it differs from a true batched-fsync design where all records in a batch are written in a single fsync call before any response is dispatched. This is not a bug but is worth noting for future optimization (PRD 004 does not require batch-fsync).

3. **`run_writer` doc comment says "for batching" but the implementation fsyncs per-item.** The comment at `writer.rs:116` says "additional pending requests are drained with `try_recv()` for batching" which implies the drain has a perf benefit; the actual implementation processes each item with a full `store.append()` (write + fsync) individually. The comment is slightly misleading -- the drain does reduce context-switch overhead (one pass through the event loop per batch) but does not batch the I/O. Minor documentation inaccuracy only.

---

## Suggestions (non-blocking)

- The two test helpers `proposed()` and `temp_store()` inside `mod tests` are defined locally in `writer.rs` and are nearly identical to helpers defined in `reader.rs`. If more test modules are added (e.g., for a `service.rs`), consider extracting these to a shared `#[cfg(test)]` utility module or a `tests/common.rs` file to avoid repetition. Not needed now.

- `ac6_durability_survives_restart` creates the `ReadIndex` directly from the reopened `Store` rather than going through `spawn_writer`. This is correct (it tests recovery, not the writer path) and is clearly intentional. A brief comment explaining this choice would help future readers understand why `spawn_writer` isn't used in the second scope.

- The `tracing::warn!` message in `run_writer` includes `req.stream_id`. This is good practice. Consider also including some correlation context (e.g., event count, event types) if the logging requirements evolve -- no action needed now.

---

## Scope Check

- **Files within scope:** YES. Only `src/writer.rs` and `src/lib.rs` were modified, exactly as specified by the ticket scope.
- **Scope creep detected:** NO. The 12 new tests are all in `src/writer.rs::tests` and cover ACs 1-9 exactly. No extra files, no new dependencies.
- **Unauthorized dependencies added:** NO.

---

## Risk Assessment

- **Regression risk:** LOW. The changes are additive: `run_writer` and `spawn_writer` are new functions; no existing code was modified. The `pub use writer::{WriterHandle, spawn_writer}` re-export is an additive change to `lib.rs`. The impl report confirms all 112 tests pass (12 new + 100 pre-existing).

- **Security concerns:** NONE. No external input surfaces, no auth, no secrets.

- **Performance concerns:** NONE at this scale. The per-item fsync design is correct per the CLAUDE.md "fsync is non-negotiable" landmine. The drain loop avoids thundering-herd overhead from repeated `recv()` yields. The `Arc<RwLock<EventLog>>` is cloned once in `spawn_writer` before the move -- correct and efficient.

- **Correctness of critical sequencing (explicit check):**
  - `store.log()` at line 180 clones the `Arc` BEFORE `store` is moved into `tokio::spawn` at line 186. Verified correct.
  - The writer loop holds no locks during `store.append()` (the write lock is acquired inside `Store::append` only after fsync, per the `store.rs` implementation verified in review). Verified correct.
  - Error from one request does not affect subsequent requests -- the `for req in batch` loop continues regardless of the `Result` returned by `store.append()`. The error is sent back to the caller; the writer stays alive. Verified correct via AC-9b test and code inspection.
  - Graceful shutdown: when all `WriterHandle` senders are dropped, `rx.recv()` returns `None`, the `while let` exits, and the task returns naturally. Verified correct.
