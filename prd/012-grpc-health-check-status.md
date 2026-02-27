# Build Status: PRD 012 -- gRPC Health Check

**Source PRD:** prd/012-grpc-health-check.md
**Tickets:** prd/012-grpc-health-check-tickets.md
**Started:** 2026-02-27 08:00
**Last Updated:** 2026-02-27 09:00
**Overall Status:** QA READY

---

## Ticket Tracker

| Ticket | Title | Status | Impl Report | Review Report | Notes |
|--------|-------|--------|-------------|---------------|-------|
| 1 | Wire tonic-health into Cargo.toml and main.rs | DONE | ticket-01-impl.md | ticket-01-review.md | APPROVED |
| 2 | Integration tests for health check | DONE | ticket-02-impl.md | ticket-02-review.md | APPROVED |
| 3 | Verification and integration check | DONE | -- | -- | All gates pass, all ACs covered |

## Prior Work Summary

- `Cargo.toml`: Added `tonic-health = "0.13"` to `[dependencies]`.
- `src/main.rs`: Constructs `(health_reporter, health_service)` via `tonic_health::server::health_reporter()`.
- `health_service` added to tonic server builder before `EventStoreServer::new(service)` (line ~299-300).
- `set_serving::<EventStoreServer<EventfoldService>>()` and `set_service_status("", Serving)` called AFTER `TcpListener::bind` (lines 325-328).
- Shutdown future transitions both service names to `NOT_SERVING` before resolving (lines 337-342).
- `health_reporter` moved into the shutdown async block (no Arc needed).
- Unit test `health_reporter_can_set_serving_and_not_serving` added in `src/main.rs`.
- 240 tests passing. Build, clippy, fmt all clean.

## Follow-Up Tickets

[None.]

## Completion Report

**Completed:** 2026-02-27 09:00
**Tickets Completed:** 3/3

### Summary of Changes

**Files created:**
- `tests/health_check.rs` -- 3 integration tests for gRPC health check service

**Files modified:**
- `Cargo.toml` -- Added `tonic-health = "0.13"` to `[dependencies]`
- `src/main.rs` -- Construct HealthReporter/HealthService, register both service names as SERVING after bind, add health service to server builder, transition to NOT_SERVING in shutdown future, 1 unit test

### Key Architectural Decisions
- `tonic-health` provides the `grpc.health.v1.Health` service out of the box, no `.proto` needed
- SERVING set after `TcpListener::bind` (not prematurely during store open)
- NOT_SERVING set inside `serve_with_incoming_shutdown` shutdown future before server drains
- `health_service` added to builder before `EventStoreServer` (convention: infrastructure services first)
- `HealthReporter` moved into shutdown closure (no Arc wrapping needed)

### AC Coverage Matrix
| AC | Description | Verified |
|----|-------------|----------|
| 1 | `cargo build` zero errors/warnings | Yes |
| 2 | `cargo clippy` passes | Yes |
| 3 | Empty service name returns SERVING | Yes (test) |
| 4 | `eventfold.EventStore` returns SERVING | Yes (test) |
| 5 | Unknown service returns NOT_FOUND | Yes (test) |
| 6 | NOT_SERVING in shutdown future | Yes (grep lines 338, 341) |
| 7 | Existing RPCs pass without modification | Yes (23/23 grpc_service tests) |
| 8 | `cargo test` all green | Yes (243 tests) |
| 9 | SERVING set after TcpListener::bind | Yes (grep lines 325, 328 after 303) |

### Ready for QA: YES
