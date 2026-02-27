# Implementation Report: Ticket 1 -- Add tonic-health dependency and wire health service into main.rs

**Ticket:** 1 - Add tonic-health dependency and wire health service into main.rs
**Date:** 2026-02-27 12:00
**Status:** COMPLETE

---

## Files Changed

### Created
- None

### Modified
- `Cargo.toml` - Added `tonic-health = "0.13"` to `[dependencies]`
- `src/main.rs` - Constructed `HealthReporter`/`HealthService` pair, added health service to server builder before `EventStoreServer`, set SERVING after listener bind, wrapped shutdown in async block that transitions to NOT_SERVING

## Implementation Notes
- `HealthReporter::set_serving()`, `set_not_serving()`, and `set_service_status()` all take `&self` (not `&mut self`), so the `health_reporter` binding does not need `mut`. The ticket AC specified `mut` but adding it would produce a compiler warning (`unused_mut`), violating the zero-warnings AC. Omitting `mut` is the correct approach.
- `health_reporter` is used by shared reference for the two `.set_serving()` / `.set_service_status()` calls after bind, then moved by value into the shutdown async block. No `Arc` wrapping needed since `HealthReporter` is cheaply cloneable internally (it wraps a `watch::Sender`).
- The `health_service` is added to the server builder unconditionally (before the TLS/non-TLS branch point), so it is registered regardless of TLS mode. This works because the builder is configured for TLS/non-TLS before `add_service()` is called.
- Added one unit test (`health_reporter_can_set_serving_and_not_serving`) that validates the `tonic-health` API patterns used in `main()` -- constructing a reporter, setting SERVING for both service names, then transitioning to NOT_SERVING.

## Acceptance Criteria
- [x] AC 1: `Cargo.toml` `[dependencies]` includes `tonic-health = "0.13"` -- Added at line 20
- [x] AC 2: `src/main.rs` calls `tonic_health::server::health_reporter()` -- Line 249 constructs `(health_reporter, health_service)`
- [x] AC 3: `set_serving` and `set_service_status("", Serving)` called after `TcpListener::bind` -- Lines 324-329, after bind at line 303
- [x] AC 4: `health_service` added to server builder before `EventStoreServer` -- Lines 298-300: `.add_service(health_service).add_service(EventStoreServer::new(service))`
- [x] AC 5: Shutdown future transitions both names to NOT_SERVING -- Lines 335-343: async block calls `set_service_status("", NotServing)` and `set_not_serving::<EventStoreServer<EventfoldService>>()`
- [x] AC 6: `HealthReporter` moved (not cloned) into shutdown future; no `Arc` wrapping -- `health_reporter` moves into the async block by value
- [x] AC 7: Existing `Config::from_env_*` tests continue to pass -- All 16 main.rs tests pass
- [x] AC 8: `cargo build` produces zero warnings -- Confirmed
- [x] AC 9: `cargo clippy --all-targets --all-features --locked -- -D warnings` passes -- Confirmed

## Test Results
- Lint: PASS (`cargo clippy --all-targets --all-features --locked -- -D warnings`)
- Tests: PASS (240 tests: 180 lib + 16 binary + 44 integration)
- Build: PASS (zero warnings)
- Format: PASS (`cargo fmt --check`)
- New tests added: `src/main.rs::tests::health_reporter_can_set_serving_and_not_serving`

## Concerns / Blockers
- None
