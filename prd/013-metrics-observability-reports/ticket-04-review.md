# Code Review: Ticket 4 -- Wire metrics into `main.rs` (Config, install_recorder, serve_metrics)

**Ticket:** 4 -- Wire metrics into `main.rs` (Config, install_recorder, serve_metrics)
**Impl Report:** prd/013-metrics-observability-reports/ticket-04-impl.md
**Date:** 2026-02-27 17:30
**Verdict:** APPROVED

---

## AC Coverage

| AC # | Description | Status | Notes |
|------|-------------|--------|-------|
| 1 | `Config` has `metrics_listen: Option<SocketAddr>` | Met | Line 51 of `src/main.rs`. Field is correctly typed and documented. |
| 2 | `Config::from_env()` reads `EVENTFOLD_METRICS_LISTEN` with four cases | Met | Lines 124-134. Three-arm match: `Ok(val) if val.is_empty() => None`, `Ok(val) => Some(parse)`, `Err(_) => Some(default)`. Invalid parse returns `Err` with message mentioning the variable name. |
| 3 | Doc comment table includes `EVENTFOLD_METRICS_LISTEN` row | Met | Line 36. Row correctly documents default `[::]:9090` and "empty disables" behavior. |
| 4 | In `main()`, after `spawn_writer`, call `install_recorder()` with error handling | Met | Lines 272-278. Called as step 7 (after step 6 `spawn_writer`). On `Err`, logs `tracing::error!` and exits with code 1. |
| 5 | If `metrics_listen` is `Some(addr)`, call `serve_metrics()` and store JoinHandle | Met | Lines 281-282. `serve_metrics(metrics_handle, addr)` called, JoinHandle stored in `Option<JoinHandle<()>>`. |
| 6 | If `metrics_listen` is `None`, log info "Metrics endpoint disabled" | Met | Line 284. `tracing::info!("Metrics endpoint disabled")` in the `else` branch. |
| 7 | In shutdown sequence, abort metrics JoinHandle if it exists | Met | Lines 394-396. `if let Some(handle) = metrics_join_handle { handle.abort(); }` -- placed after gRPC server exits, before writer handle drop. |
| 8 | Test `from_env_metrics_listen_default` | Met | Lines 752-766. Asserts `config.metrics_listen == Some("[::]:9090".parse().unwrap())` when env var is unset. |
| 9 | Test `from_env_metrics_listen_empty_string_gives_none` | Met | Lines 770-781. Sets env var to `""`, asserts `config.metrics_listen == None`. |
| 10 | Test `from_env_metrics_listen_custom_addr` | Met | Lines 785-799. Sets env var to `"127.0.0.1:19090"`, asserts parsed correctly. |
| 11 | Test `from_env_metrics_listen_invalid_addr` | Met | Lines 801-822. Sets env var to `"not-an-addr"`, asserts `Err` with message containing `"EVENTFOLD_METRICS_LISTEN"`. |
| 12 | All existing `from_env_*` tests still pass; each `#[serial]` test clears `EVENTFOLD_METRICS_LISTEN` | Met | All 12 pre-existing `#[serial]` tests call `clear_metrics_env()`. Both subprocess tests (`binary_exits_nonzero_*`) use `.env_remove("EVENTFOLD_METRICS_LISTEN")`. All 4 new serial tests also properly handle the env var. Verified: `cargo test` passes all 254 tests. |
| 13 | `cargo build` produces zero warnings | Met | Verified: `cargo build` clean. |
| 14 | `cargo clippy --all-targets --all-features --locked -- -D warnings` passes | Met | Verified: clippy passes clean. |

## Issues Found

### Critical (must fix before merge)
None.

### Major (should fix, risk of downstream problems)
None.

### Minor (nice to fix, not blocking)
- The `from_env_defaults_when_only_data_set` test (lines 425-442) clears `EVENTFOLD_METRICS_LISTEN` but does not assert on `config.metrics_listen`. While the dedicated `from_env_metrics_listen_default` test covers this, adding `assert_eq!(config.metrics_listen, Some("[::]:9090".parse().unwrap()))` to the "defaults" test would make it more comprehensive. Not blocking.

## Suggestions (non-blocking)
- The impl report claims `src/service.rs` was "Reformatted by `cargo fmt` (pre-existing formatting issues from a parallel ticket; no logic changes)." This is misleading -- the `service.rs` diff against HEAD includes 277 lines of additions from Ticket 3 (counter/gauge instrumentation, `SubscriptionGauge` struct, 3 new async tests). The Ticket 4 implementer likely meant "I ran `cargo fmt` which may have reformatted service.rs beyond what Ticket 3 left." In future reports, clarifying that the working tree contains uncommitted changes from prior tickets would reduce confusion.
- The step numbers in `main()` were renumbered from 7 onwards (7-13 became 7-15). This is clean and consistent.

## Scope Check
- Files within scope: YES. `src/main.rs` is the only file in Ticket 4's scope.
- Scope creep detected: NO. The `src/service.rs` changes visible in `git diff` are from Ticket 3 (confirmed by Ticket 3's impl report). The Ticket 4 implementer's contribution to service.rs is at most `cargo fmt` reformatting, which is a legitimate cleanup necessary to pass the `cargo fmt --check` gate.
- Unauthorized dependencies added: NO.

## Risk Assessment
- Regression risk: LOW. The `Config` change is additive (new field with a sensible default). All 16 pre-existing `#[serial]` tests and both subprocess tests now clean the new env var, preventing env leakage. The shutdown sequence correctly handles both `Some` and `None` paths.
- Security concerns: NONE. No secrets, no user input handling.
- Performance concerns: NONE. `install_recorder()` and `serve_metrics()` are one-time startup operations.
