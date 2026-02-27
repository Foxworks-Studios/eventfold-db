# Implementation Report: Ticket 4 -- Wire metrics into `main.rs` (Config, install_recorder, serve_metrics)

**Ticket:** 4 - Wire metrics into `main.rs` (Config, install_recorder, serve_metrics)
**Date:** 2026-02-27 16:45
**Status:** COMPLETE

---

## Files Changed

### Created
- None

### Modified
- `src/main.rs` - Added `metrics_listen: Option<SocketAddr>` field to `Config`, added `DEFAULT_METRICS_LISTEN_ADDR` constant, added `EVENTFOLD_METRICS_LISTEN` env var parsing in `from_env()`, added `install_recorder()` and conditional `serve_metrics()` calls in `main()`, added metrics JoinHandle abort in shutdown sequence, updated doc comment table, added `clear_metrics_env()` helper and 4 new tests, updated all existing serial tests to clear `EVENTFOLD_METRICS_LISTEN`
- `src/service.rs` - Reformatted by `cargo fmt` (pre-existing formatting issues from a parallel ticket; no logic changes)

## Implementation Notes
- Followed the existing env var parsing pattern exactly: `match std::env::var(...)` with `Ok(val)` for set, `Err(_)` for unset, and a match guard `Ok(val) if val.is_empty()` for the empty-string-disables case.
- The `metrics_listen` field is placed after `tls` in the struct, and its parsing is placed before the TLS block in `from_env()` since it is simpler.
- Step numbers in `main()` were renumbered from 7 onwards to accommodate the two new steps (install recorder at step 7, optional serve at step 8).
- The metrics JoinHandle is aborted in the shutdown sequence after the gRPC server exits but before the writer handle is dropped, matching the ticket requirement.
- All 16 existing `#[serial]` tests now call `clear_metrics_env()` to prevent `EVENTFOLD_METRICS_LISTEN` from leaking between tests.
- The two subprocess-based tests (`binary_exits_nonzero_*`) use `.env_remove("EVENTFOLD_METRICS_LISTEN")` to prevent env leakage to child processes.
- `cargo fmt` also reformatted `src/service.rs` which had pre-existing formatting issues from a parallel ticket. This is noted as an out-of-scope change.

## Acceptance Criteria
- [x] AC 1: `Config` struct has field `metrics_listen: Option<SocketAddr>` -- added at line 51
- [x] AC 2: `Config::from_env()` reads `EVENTFOLD_METRICS_LISTEN` with all four cases (not set -> default, empty -> None, valid addr -> Some, invalid -> Err) -- implemented at lines 124-134
- [x] AC 3: Doc comment table on `Config` includes `EVENTFOLD_METRICS_LISTEN` row -- added at line 36
- [x] AC 4: In `main()`, after `spawn_writer`, call `install_recorder()` with error handling -- implemented at lines 272-278
- [x] AC 5: If `metrics_listen` is `Some(addr)`, call `serve_metrics()` and store JoinHandle -- implemented at lines 281-283
- [x] AC 6: If `metrics_listen` is `None`, log info "Metrics endpoint disabled" -- implemented at line 284
- [x] AC 7: In shutdown sequence, abort metrics JoinHandle if it exists -- implemented at lines 394-396
- [x] AC 8: Test `from_env_metrics_listen_default` -- added at lines 750-766
- [x] AC 9: Test `from_env_metrics_listen_empty_string_gives_none` -- added at lines 768-781
- [x] AC 10: Test `from_env_metrics_listen_custom_addr` -- added at lines 783-799
- [x] AC 11: Test `from_env_metrics_listen_invalid_addr` -- added at lines 801-822
- [x] AC 12: All existing `from_env_*` tests still pass; each `#[serial]` test clears `EVENTFOLD_METRICS_LISTEN` -- verified, all 20 binary tests pass
- [x] AC 13: `cargo build` produces zero warnings -- verified
- [x] AC 14: `cargo clippy --all-targets --all-features --locked -- -D warnings` passes -- verified

## Test Results
- Lint: PASS (clippy clean)
- Tests: PASS (254 total: 187 lib + 20 binary + 2 + 23 + 3 + 6 + 6 + 6 + 1 + 0 doc-tests)
- Build: PASS (zero warnings)
- Format: PASS
- New tests added:
  - `src/main.rs::tests::from_env_metrics_listen_default`
  - `src/main.rs::tests::from_env_metrics_listen_empty_string_gives_none`
  - `src/main.rs::tests::from_env_metrics_listen_custom_addr`
  - `src/main.rs::tests::from_env_metrics_listen_invalid_addr`

## Concerns / Blockers
- `src/service.rs` had pre-existing `cargo fmt` violations (from a parallel ticket). Running `cargo fmt` fixed them. This is noted as an out-of-scope file change that was necessary to pass the `cargo fmt --check` gate.
- None other.
