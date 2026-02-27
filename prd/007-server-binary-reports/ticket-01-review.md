# Code Review: Ticket 1 -- Add `tracing-subscriber` Dependency and `Config` Struct

**Ticket:** 1 -- Add `tracing-subscriber` Dependency and `Config` Struct
**Impl Report:** prd/007-server-binary-reports/ticket-01-impl.md
**Date:** 2026-02-26 14:30
**Verdict:** APPROVED

---

## AC Coverage

| AC # | Description | Status | Notes |
|------|-------------|--------|-------|
| 1 | `Cargo.toml` contains `tracing-subscriber` with `env-filter` feature | Met | Line 18 of `Cargo.toml`: `tracing-subscriber = { version = "0.3", features = ["env-filter"] }` |
| 2 | `Config` struct has three fields: `data_path: PathBuf`, `listen_addr: SocketAddr`, `broker_capacity: usize` | Met | Lines 15-22 of `src/main.rs`. All three fields present with correct types. |
| 3 | `Config::from_env()` returns `Err(String)` containing `"EVENTFOLD_DATA"` when not set | Met | Line 52: error message is `"EVENTFOLD_DATA environment variable is required"`. Test `from_env_missing_data_returns_err` (line 122) asserts `msg.contains("EVENTFOLD_DATA")`. |
| 4 | Default `listen_addr` is `[::]:2113` when `EVENTFOLD_LISTEN` not set | Met | Lines 58-60: falls through to `DEFAULT_LISTEN_ADDR` constant (`"[::]:2113"`, line 26). Test `from_env_defaults_when_only_data_set` (line 115) asserts exact match. |
| 5 | Default `broker_capacity` is `4096` when `EVENTFOLD_BROKER_CAPACITY` not set | Met | Line 67: falls through to `DEFAULT_BROKER_CAPACITY` constant (`4096`, line 30). Test `from_env_defaults_when_only_data_set` (line 117) asserts `== 4096`. |
| 6 | Invalid `EVENTFOLD_LISTEN` returns `Err(String)` | Met | Lines 56-57: `parse::<SocketAddr>()` error mapped to descriptive `Err(String)`. Test `from_env_invalid_listen_addr_returns_err` (line 166) confirms. |
| 7 | Invalid `EVENTFOLD_BROKER_CAPACITY` returns `Err(String)` | Met | Lines 65-66: `parse::<usize>()` error mapped to descriptive `Err(String)`. Test `from_env_invalid_broker_capacity_returns_err` (line 185) confirms. |
| 8 | `init_tracing()` initializes subscriber with `EnvFilter` from `RUST_LOG` (default `"info"`), idempotent | Met | Lines 84-92: `EnvFilter::try_from_default_env()` with `"info"` fallback; `try_init()` makes it idempotent. Test `init_tracing_does_not_panic` (line 177) exercises it. |
| 9 | Test: defaults with only `EVENTFOLD_DATA` set | Met | `from_env_defaults_when_only_data_set` (line 105) verifies all three fields. |
| 10 | Test: missing `EVENTFOLD_DATA` returns Err | Met | `from_env_missing_data_returns_err` (line 122). |
| 11 | Test: custom `EVENTFOLD_LISTEN` | Met | `from_env_custom_listen_addr` (line 139). |
| 12 | Test: custom `EVENTFOLD_BROKER_CAPACITY` | Met | `from_env_custom_broker_capacity` (line 154). |
| 13 | Test: invalid listen address returns Err | Met | `from_env_invalid_listen_addr_returns_err` (line 166). |
| 14 | Quality gates pass | Met | Verified independently: `cargo build` (0 warnings), `cargo clippy --all-targets --all-features --locked -- -D warnings` (clean), `cargo fmt --check` (clean), `cargo test` (183 passed, 0 failed). |

## Issues Found

### Critical (must fix before merge)
- None.

### Major (should fix, risk of downstream problems)
- None.

### Minor (nice to fix, not blocking)
- **Missing env var cleanup in tests.** The `#[serial]` tests set env vars (`EVENTFOLD_DATA`, `EVENTFOLD_LISTEN`, `EVENTFOLD_BROKER_CAPACITY`) but do not restore/remove them after the test body. This is safe because `#[serial]` guarantees no concurrent test execution within the serial group, and each test explicitly sets or removes the vars it needs. However, if a non-serial test (e.g., `init_tracing_does_not_panic`) runs after a serial test, env vars from the previous serial test remain in the process environment. In this case it is harmless because `init_tracing_does_not_panic` does not read config vars, but future tests could be affected. Consider adding cleanup or documenting the convention.

## Suggestions (non-blocking)
- The `#[allow(dead_code)]` annotations (lines 13, 25, 29, 32, 83) are well-justified and clearly commented with "Wired into main() by Ticket 2." No action needed now; just confirm they are removed in Ticket 2's review.
- The `serial_test` dev-dependency addition is reasonable and necessary for Rust 2024 edition's `unsafe` requirement on `set_var`/`remove_var`. Well-documented in the impl report.
- The `.expect("default listen address is valid")` on line 60 is correct usage: this is a compile-time-verifiable invariant (the constant `"[::]:2113"` is known-valid), and panicking here would indicate a programmer error, not an operational failure. Consistent with CLAUDE.md conventions.

## Scope Check
- Files within scope: YES. Only `Cargo.toml` and `src/main.rs` were modified, as specified in the ticket.
- Scope creep detected: MINOR. `serial_test = "3"` was added to `[dev-dependencies]`, which was not listed in the ticket scope. However, this is a test infrastructure dependency required by the Rust 2024 edition's safety model for env var mutation. The implementer documented the rationale clearly. This is justified and does not constitute problematic scope creep.
- Unauthorized dependencies added: NO. `tracing-subscriber` was explicitly required. `serial_test` is a well-known test utility and its addition is justified.

## Risk Assessment
- Regression risk: LOW. No existing code was modified (only the placeholder `main()` remains unchanged). All 183 existing + new tests pass. The new `Config` struct and `init_tracing()` are `#[allow(dead_code)]` and not yet wired into any production path.
- Security concerns: NONE. No secrets, no network I/O, no user-facing input handling (env vars are trusted process-level configuration).
- Performance concerns: NONE. `Config::from_env()` does three env var reads and two string parses. `init_tracing()` runs once at startup.
