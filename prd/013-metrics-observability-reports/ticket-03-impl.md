# Implementation Report: Ticket 3 -- Instrument `service.rs` with read and subscription metrics

**Ticket:** 3 - Instrument `service.rs` with read and subscription metrics
**Date:** 2026-02-27 14:30
**Status:** COMPLETE

---

## Files Changed

### Modified
- `src/service.rs` - Added `counter!` calls to `read_stream` and `read_all` handlers; added `SubscriptionGauge` RAII guard for `subscribe_all` and `subscribe_stream`; added 3 new tests for metric instrumentation.
- `src/metrics.rs` - Added `pub fn get_installed_handle()` accessor for the `OnceLock`-stored `MetricsHandle` (needed by service tests to render metrics for assertions).

## Implementation Notes
- **Counter placement**: `counter!("eventfold_reads_total", "rpc" => "read_stream").increment(1)` is the very first statement in `read_stream`, before `request.into_inner()` or any parsing. Same for `read_all`. This ensures the counter fires on every call regardless of outcome.
- **Gauge guard pattern**: Instead of manually placing increment/decrement calls, I created a `SubscriptionGauge` struct that increments the `eventfold_subscriptions_active` gauge on construction and decrements on drop. This guarantees the decrement fires even when the client disconnects mid-stream, because the `async_stream::stream!` generator's local variables are dropped when the stream is dropped. The guard is the first local variable in the `stream!` block.
- **metrics crate 0.24 API**: Uses `counter!("name", "key" => "value").increment(N)` and `gauge!("name").increment(f64)` / `gauge!("name").decrement(f64)` -- the two-step register-then-mutate API, not the older single-call syntax.
- **Test delta pattern**: Tests snapshot the metric value before and after operations to compute the delta, making them robust to global state accumulated by other tests in the same process. Tests use `#[serial]` from `serial_test` to avoid parallel interference on the shared global metrics recorder.
- **Out-of-scope modification**: Added `get_installed_handle()` to `src/metrics.rs` because the `RECORDER_HANDLE` `OnceLock` is private and there was no public way to retrieve the handle after `install_recorder()` returned `Err(AlreadyInstalled)`. This 4-line function is a pure accessor with no side effects and is necessary for the test AC to render metrics.

## Acceptance Criteria
- [x] AC 1: In `read_stream`, a reads_total counter with label `rpc="read_stream"` is incremented at the top of the handler body, before any early-return on error -- `counter!("eventfold_reads_total", "rpc" => "read_stream").increment(1)` on line 123, first statement in the handler.
- [x] AC 2: In `read_all`, a reads_total counter with label `rpc="read_all"` is incremented at the top of the handler body, before any early-return on error -- `counter!("eventfold_reads_total", "rpc" => "read_all").increment(1)` on line 146, first statement in the handler.
- [x] AC 3: In `subscribe_all`, a subscriptions_active gauge is incremented (+1.0) at the start and decremented (-1.0) after the stream finishes. The decrement fires even when the client disconnects mid-stream -- via `SubscriptionGauge` RAII guard in the `stream!` block.
- [x] AC 4: Same pattern for `subscribe_stream` -- identical `SubscriptionGauge` guard placement.
- [x] AC 5: Test for read counters -- `read_handlers_increment_reads_total_counter` calls `read_stream` once and `read_all` twice, then asserts delta of 1 and 2 respectively.
- [x] AC 6: Test for subscription gauge -- `subscribe_all_increments_and_decrements_gauge` verifies gauge goes to +1 while stream is open and back to 0 after drop. `subscribe_stream_increments_and_decrements_gauge` does the same for `subscribe_stream`.
- [x] AC 7: `cargo build` produces zero warnings.
- [x] AC 8: `cargo clippy --all-targets --all-features --locked -- -D warnings` passes clean.

## Test Results
- Lint: PASS (`cargo clippy --all-targets --all-features --locked -- -D warnings` clean)
- Tests: PASS (254 total tests, 0 failures)
- Build: PASS (zero warnings)
- Format: PASS (`cargo fmt --check` clean)
- New tests added:
  - `src/service.rs::tests::read_handlers_increment_reads_total_counter`
  - `src/service.rs::tests::subscribe_all_increments_and_decrements_gauge`
  - `src/service.rs::tests::subscribe_stream_increments_and_decrements_gauge`

## Concerns / Blockers
- **Out-of-scope file modified**: `src/metrics.rs` was modified to add `get_installed_handle()`. This was not listed in the ticket scope but was necessary for the test AC (rendering metrics from `service.rs` tests). The function is a trivial accessor (4 lines + doc comment) with no behavioral change. The reviewer should confirm this is acceptable.
- None otherwise.
