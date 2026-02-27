# Code Review: Ticket 3 -- Instrument `service.rs` with read and subscription metrics

**Ticket:** 3 -- Instrument `service.rs` with read and subscription metrics
**Impl Report:** prd/013-metrics-observability-reports/ticket-03-impl.md
**Date:** 2026-02-27 17:00
**Verdict:** APPROVED

---

## AC Coverage

| AC # | Description | Status | Notes |
|------|-------------|--------|-------|
| 1 | `read_stream` counter with label `rpc="read_stream"` at top of handler, before any early return | Met | `src/service.rs` line 123: `counter!("eventfold_reads_total", "rpc" => "read_stream").increment(1)` is the first statement in the handler body, before `request.into_inner()` (line 124) and `parse_uuid` (line 126). Uses metrics 0.24 two-step API rather than AC's single-call syntax -- functionally equivalent, correct for the installed crate version. |
| 2 | `read_all` counter with label `rpc="read_all"` at top of handler, before any early return | Met | `src/service.rs` line 146: `counter!("eventfold_reads_total", "rpc" => "read_all").increment(1)` is the first statement in the handler body, before `request.into_inner()` (line 147). `read_all` has no early-return error paths, but the counter is still placed first as specified. |
| 3 | `subscribe_all` gauge increments at start, decrements after stream finishes, guaranteed decrement on client disconnect | Met | `src/service.rs` lines 176-177: `SubscriptionGauge::new()` RAII guard is the first variable in the `async_stream::stream!` block. `SubscriptionGauge` (lines 282-296) increments on `new()` and decrements on `Drop`. The guard is dropped when the stream generator returns or when tonic drops the stream on client disconnect. This is equivalent to a `scopeguard` as allowed by the AC. |
| 4 | Same gauge pattern for `subscribe_stream` | Met | `src/service.rs` lines 235-236: Identical `SubscriptionGauge::new()` guard as first variable in the `stream!` block for `subscribe_stream`. Symmetrical implementation. |
| 5 | Test: read_stream once + read_all twice, assert counter deltas | Met | `src/service.rs` lines 662-739: `read_handlers_increment_reads_total_counter` creates a temp service, appends one event (to create a valid stream), calls `read_stream` once and `read_all` twice, renders metrics, and asserts delta of 1 and 2 respectively. Uses before/after snapshot pattern for robustness against global state. AC says "valid but empty stream" but a stream with one event is equally valid for counting purposes. |
| 6 | Test: subscription gauge increments while open, decrements on drop | Met | `src/service.rs` lines 741-793 (`subscribe_all_increments_and_decrements_gauge`): Snapshots gauge, opens `subscribe_all`, polls CaughtUp to ensure stream is active, asserts +1 delta, drops stream, `yield_now()`, asserts delta returns to 0. Lines 795-865 (`subscribe_stream_increments_and_decrements_gauge`): Same pattern for `subscribe_stream` -- creates a stream, appends an event, opens subscription, polls first event, asserts +1, drops, asserts 0. Both tests correctly verify the RAII guard pattern. |
| 7 | `cargo build` zero warnings | Met | Verified: `cargo build` completes with zero warnings. The Ticket 2 impl report noted pre-existing warnings from unused `gauge`/`serial`/`SubscriptionGauge` in service.rs -- Ticket 3 resolves those by activating the code. |
| 8 | `cargo clippy` passes | Met | Verified: `cargo clippy --all-targets --all-features --locked -- -D warnings` passes clean with zero diagnostics. |

## Issues Found

### Critical (must fix before merge)

None.

### Major (should fix, risk of downstream problems)

None.

### Minor (nice to fix, not blocking)

- **`tokio::task::yield_now()` for drop propagation** (`src/service.rs` lines 783, 855): After `drop(stream)`, a single `yield_now()` is used to allow the runtime to run the stream's cleanup path. This is sufficient because `async_stream::stream!` generators drop their locals synchronously when the stream future is dropped. However, if the stream internals ever change to require multiple poll cycles for cleanup, this could become flaky. A `tokio::time::sleep(Duration::from_millis(10))` would be more defensive but slower. Current approach is acceptable and consistent with the test patterns used elsewhere in the codebase.

## Suggestions (non-blocking)

- The `ensure_metrics_handle()` helper and `temp_service()` helper in `service.rs` tests are well-designed and could be reused by Ticket 5's integration tests (with some adaptation). Consider extracting `temp_service()` to a shared test utility if the integration test ends up needing a similar setup.
- The `parse_metric_value()` helper is a useful pattern. If it's needed in other test modules (e.g., `writer.rs` tests, integration tests), consider moving it to a shared test utility.

## Scope Check

- Files within scope: PARTIAL -- `src/service.rs` is explicitly in scope per the ticket.
- **`src/metrics.rs` modified (out of scope):** The ticket scope says "Modify: `src/service.rs`" only. The implementer added `pub fn get_installed_handle() -> Option<MetricsHandle>` (12 lines including doc comment, lines 82-93 of `src/metrics.rs`). The impl report explicitly flagged this as an out-of-scope modification and justified it: the `RECORDER_HANDLE` `OnceLock` was private, and there was no public accessor to retrieve the handle after `install_recorder()` returned `Err(AlreadyInstalled)`. The function is a pure accessor with no side effects. **This is acceptable** -- it's a trivial, necessary accessor that enables the test ACs. The alternative would have been to make the `OnceLock` public, which is worse.
- Scope creep detected: NO -- beyond the `get_installed_handle()` accessor, no extra features or abstractions were introduced.
- Unauthorized dependencies added: NO

## Risk Assessment

- Regression risk: LOW -- Existing handlers are unchanged except for the addition of a single `counter!` call as the first statement. The `SubscriptionGauge` guard is inside the `stream!` block and cannot affect handler logic. All 254 tests pass, including all pre-existing integration tests.
- Security concerns: NONE -- Counter and gauge macros have negligible side effects (atomic increments). No new data exposure.
- Performance concerns: NONE -- `counter!()` and `gauge!()` are near-zero-cost atomic operations. The `SubscriptionGauge` RAII guard is a ZST (zero-sized type) with no heap allocation. The `Drop` impl is a single atomic decrement.

## Verification

All quality gates confirmed passing:
- `cargo build`: zero warnings
- `cargo clippy --all-targets --all-features --locked -- -D warnings`: zero diagnostics
- `cargo test`: 254 tests, all green (187 unit + 67 integration/other), including 3 new metrics tests
- `cargo fmt --check`: no diff
