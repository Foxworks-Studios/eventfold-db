# Build Status: PRD 014 -- ListStreams RPC

**Source PRD:** prd/014-list-streams-rpc.md
**Tickets:** prd/014-list-streams-rpc-tickets.md
**Started:** 2026-02-27 14:00
**Last Updated:** 2026-02-27 15:00
**Overall Status:** QA READY

---

## Ticket Tracker

| Ticket | Title | Status | Impl Report | Review Report | Notes |
|--------|-------|--------|-------------|---------------|-------|
| 1 | Add `StreamInfo` domain type and re-export | DONE | ticket-01-impl.md | -- | 283 tests |
| 2 | Add `ListStreams` messages and RPC to proto | DONE | ticket-02-impl.md | -- | 288 tests; stub handler added to service.rs |
| 3 | Implement `ReadIndex::list_streams` | DONE | ticket-03-impl.md | -- | 288 tests; 4 new unit tests |
| 4 | Implement `list_streams` gRPC handler | DONE | ticket-04-impl.md | -- | 291 tests; 3 new tests |
| 5 | Refactor `eventfold-console` | DONE | ticket-05-impl.md | -- | collect_streams removed, ListStreams RPC wired |
| 6 | Integration tests and verification | DONE | ticket-06-impl.md | -- | 295 tests; 4 new integration tests |

## Prior Work Summary

- `src/types.rs`: Added `StreamInfo` struct with `stream_id: Uuid`, `event_count: u64`, `latest_version: u64`; derives `Debug, Clone, PartialEq, Eq`. Re-exported from `src/lib.rs`.
- `proto/eventfold.proto`: Added `rpc ListStreams(ListStreamsRequest) returns (ListStreamsResponse)`, `ListStreamsRequest`, `StreamInfo`, `ListStreamsResponse` messages.
- `src/reader.rs`: Added `ReadIndex::list_streams()` returning `Vec<StreamInfo>` sorted lexicographically by `stream_id.to_string()`. Single read-lock, O(s) complexity. 4 unit tests.
- `src/service.rs`: Added `list_streams` handler + `stream_info_to_proto` conversion helper. 3 unit tests.
- `eventfold-console/src/client.rs`: Replaced `ReadAll` scan loop with single `ListStreams` RPC call. Removed `page_size` parameter.
- `eventfold-console/src/app.rs`: Deleted `collect_streams` method and 2 unit tests.
- `eventfold-console/src/main.rs`: Removed `LIST_STREAMS_PAGE_SIZE` constant, updated call site.
- `tests/grpc_service.rs`: 4 new integration tests (empty store, multi-stream metadata, sort order, live index).
- 295 tests passing. Build, clippy, fmt all clean.

## Follow-Up Tickets

[None.]

## Completion Report

**Completed:** 2026-02-27 15:00
**Tickets Completed:** 6/6

### Summary of Changes

**Files modified:**
- `src/types.rs` -- Added `StreamInfo` struct + 2 unit tests
- `src/lib.rs` -- Re-exported `StreamInfo`, added proto compile-time test
- `proto/eventfold.proto` -- Added `ListStreams` RPC, 3 new messages
- `src/reader.rs` -- Added `ReadIndex::list_streams()` + 4 unit tests
- `src/service.rs` -- Added `list_streams` handler + `stream_info_to_proto` + 3 unit tests
- `eventfold-console/src/client.rs` -- Replaced ReadAll scan with ListStreams RPC
- `eventfold-console/src/app.rs` -- Deleted `collect_streams` + 2 tests
- `eventfold-console/src/main.rs` -- Removed `LIST_STREAMS_PAGE_SIZE`, updated call site
- `tests/grpc_service.rs` -- 4 new integration tests

### Key Architectural Decisions
- `ReadIndex::list_streams()` reads only from `EventLog::streams` HashMap, never touches event data -- O(s) not O(n)
- Results sorted lexicographically by UUID string for deterministic ordering
- `ListStreamsRequest` is an empty message (not reusing `Empty`) for future extensibility
- Console maps proto `StreamInfo` to its local `app::StreamInfo` (String-based stream_id)

### AC Coverage Matrix
| AC | Verified |
|----|----------|
| 1 | Yes -- proto defines rpc, request, response, StreamInfo messages |
| 2 | Yes -- integration test verifies N entries with correct fields |
| 3 | Yes -- integration test verifies empty store returns OK with empty list |
| 4 | Yes -- integration test verifies lexicographic sort order |
| 5 | Yes -- ReadIndex::list_streams uses single read lock, no event data access |
| 6 | Yes -- Client::list_streams calls ListStreams RPC, no ReadAll |
| 7 | Yes -- collect_streams deleted, zero references in source |
| 8 | Yes -- cargo build zero warnings for both crates |
| 9 | Yes -- cargo clippy clean |
| 10 | Yes -- 295 tests all green |

### Ready for QA: YES
