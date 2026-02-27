# Code Review (Round 2): Ticket 2 -- Add `Store::log_file_len()` and instrument `run_writer`

**Ticket:** 2 -- Add `Store::log_file_len()` and instrument `run_writer` with metrics
**Impl Report:** prd/013-metrics-observability-reports/ticket-02-impl.md
**Date:** 2026-02-27 17:15
**Verdict:** APPROVED

---

## Round 2 Summary

Round 1 identified one Critical issue: AC 11 (writer metrics unit test) was missing. The implementer has added the test `ac11_writer_metrics_appends_and_events_total` in `src/writer.rs` along with a `parse_counter` helper. This round verifies the fix and re-confirms all other ACs still hold.

## AC Coverage

| AC # | Description | Status | Notes |
|------|-------------|--------|-------|
| 1 | `Store::log_file_len(&self) -> Result<u64, Error>` public method using `self.file.metadata()` | Met | `src/store.rs:588` -- `Ok(self.file.metadata()?.len())`. Unchanged from Round 1. |
| 2 | Doc comment with `# Arguments`, `# Returns`, `# Errors` sections | Partial | Doc comment at lines 576-587 has `# Returns` and `# Errors` but omits `# Arguments`. Since `&self` has nothing to document, this is defensible. See Minor note. |
| 3 | Test: fresh store `log_file_len()` returns `Ok(n)` where `n >= 8` | Met | Test `log_file_len_fresh_store_returns_at_least_header_size` at line 2234. |
| 4 | Test: `log_file_len()` grows after append | Met | Test `log_file_len_grows_after_append` at line 2247. |
| 5 | `Instant::now()` before `store.append()`, `elapsed()` after `Ok`, histogram `"eventfold_append_duration_seconds"` | Met | Lines 216, 222-223 of `src/writer.rs`. Only on success path. |
| 6 | `counter!("eventfold_appends_total")` incremented once per request | Met | Line 224: `counter!("eventfold_appends_total").increment(1)`. |
| 7 | `counter!("eventfold_events_total")` incremented by `recorded.len()` | Met | Line 225: `counter!("eventfold_events_total").increment(recorded.len() as u64)`. |
| 8 | Stream count from `EventLog` via read lock, `gauge!("eventfold_streams_total")` | Met | Lines 228-232: acquires read lock, reads `streams.len()`, sets gauge. |
| 9 | `store.log_file_len()` with `Err` -> `tracing::warn!`, skip gauge (no panic) | Met | Lines 234-241: `match` with `Ok` setting gauge, `Err` logging warning. |
| 10 | `gauge!("eventfold_global_position")` set to `(recorded.last().global_position + 1) as f64` | Met | Lines 244-246: guarded by `if let Some(last)`. |
| 11 | Test in `writer.rs #[cfg(test)]`: append 3 events, render metrics, assert `appends_total=3` and `events_total=3` | **Met** | New test `ac11_writer_metrics_appends_and_events_total` at line 1225. Uses delta-based pattern (snapshots before/after), `#[serial_test::serial]` for process-global recorder isolation, clean writer shutdown before final snapshot. Verified passing: `cargo test ac11_writer_metrics` green. |
| 12 | `cargo build` produces zero warnings | Met | Verified: zero warnings. |
| 13 | `cargo clippy --all-targets --all-features --locked -- -D warnings` passes | Met | Verified: clean. |

## Issues Found

### Critical (must fix before merge)

- None. The Round 1 critical (missing AC 11 test) has been resolved.

### Major (should fix, risk of downstream problems)

- None.

### Minor (nice to fix, not blocking)

- **Missing `# Arguments` section in `log_file_len` doc comment** (`src/store.rs:576-587`). Carried forward from Round 1. The method takes only `&self`, so this is a reasonable omission consistent with other methods in the file.

## Suggestions (non-blocking)

- The `parse_counter` helper (`src/writer.rs:1197-1223`) handles labels, floats, and comment lines well. One minor observation: if a metric name is a prefix of another metric name (e.g., `eventfold_appends_total` vs `eventfold_appends_total_extra`), the `strip_prefix` + check for digit/`{`/empty correctly rejects the false match because the underscore after `total` would not pass the guard at line 1208. This is sound as-is.

## Scope Check
- Files within scope: YES -- only `src/writer.rs` was modified in this round (test module only). `src/store.rs` unchanged from Round 1.
- Scope creep detected: NO
- Unauthorized dependencies added: NO -- `serial_test` was already a dev-dependency.

## Risk Assessment
- Regression risk: LOW -- 255 tests all pass. The new test is additive and `#[serial]`-annotated to avoid global recorder conflicts.
- Security concerns: NONE
- Performance concerns: NONE

## Verification Evidence

```
cargo test                 -- 255 tests, 0 failures
cargo clippy ... -D warnings  -- 0 diagnostics
cargo fmt --check             -- clean
cargo test ac11_writer_metrics -- 1 test, passed
```
