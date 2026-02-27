# Implementation Report: Ticket 1 -- Add `tracing-subscriber` Dependency and `Config` Struct

**Ticket:** 1 - Add `tracing-subscriber` Dependency and `Config` Struct
**Date:** 2026-02-26 12:00
**Status:** COMPLETE

---

## Files Changed

### Created
- None

### Modified
- `Cargo.toml` - Added `tracing-subscriber = { version = "0.3", features = ["env-filter"] }` to `[dependencies]` and `serial_test = "3"` to `[dev-dependencies]`
- `src/main.rs` - Replaced placeholder with `Config` struct, `Config::from_env()`, `init_tracing()`, and 7 unit tests

## Implementation Notes
- **Config struct** has three fields (`data_path: PathBuf`, `listen_addr: SocketAddr`, `broker_capacity: usize`) as specified. It is `crate`-private (no `pub`) since it lives in `main.rs` and is only consumed by the binary's startup sequence.
- **`Config::from_env()`** reads `EVENTFOLD_DATA` (required), `EVENTFOLD_LISTEN` (defaults to `[::]:2113`), and `EVENTFOLD_BROKER_CAPACITY` (defaults to `4096`). Returns `Err(String)` with descriptive messages on invalid input.
- **`init_tracing()`** uses `tracing_subscriber::fmt()` with `EnvFilter::try_from_default_env()`, falling back to `"info"`. Uses `try_init()` for idempotency (repeated calls in tests do not panic).
- **`#[allow(dead_code)]`** annotations on `Config`, its impl block, the constants, and `init_tracing()` because these are foundation items not yet wired into `main()` (Ticket 2's responsibility). This follows the established binary crate pattern.
- **`serial_test = "3"`** added as a dev-dependency (not in the ticket's explicit scope, but necessary for safe env var mutation in Rust 2024 edition where `set_var`/`remove_var` are `unsafe`). All env-mutating tests use `#[serial]` and `// SAFETY: serial test` comments.
- **`main()` is still a placeholder** (`println!("Hello, world!")`). Ticket 2 will replace it with the full startup sequence.

## Acceptance Criteria
- [x] AC: `Cargo.toml` `[dependencies]` contains `tracing-subscriber` with the `env-filter` feature - Line 18 of Cargo.toml
- [x] AC: `Config` struct has three fields: `data_path: PathBuf`, `listen_addr: SocketAddr`, `broker_capacity: usize` - Lines 15-21 of main.rs
- [x] AC: `Config::from_env()` returns `Err(String)` containing `"EVENTFOLD_DATA"` when not set - Tested by `from_env_missing_data_returns_err`
- [x] AC: Default `listen_addr` is `[::]:2113` when `EVENTFOLD_LISTEN` not set - Tested by `from_env_defaults_when_only_data_set`
- [x] AC: Default `broker_capacity` is `4096` when `EVENTFOLD_BROKER_CAPACITY` not set - Tested by `from_env_defaults_when_only_data_set`
- [x] AC: Invalid `EVENTFOLD_LISTEN` returns `Err(String)` - Tested by `from_env_invalid_listen_addr_returns_err`
- [x] AC: Invalid `EVENTFOLD_BROKER_CAPACITY` returns `Err(String)` - Tested by `from_env_invalid_broker_capacity_returns_err`
- [x] AC: `init_tracing()` initializes subscriber with EnvFilter, idempotent - Tested by `init_tracing_does_not_panic`
- [x] Test: defaults with only EVENTFOLD_DATA set - `from_env_defaults_when_only_data_set`
- [x] Test: missing EVENTFOLD_DATA returns Err - `from_env_missing_data_returns_err`
- [x] Test: custom EVENTFOLD_LISTEN - `from_env_custom_listen_addr`
- [x] Test: custom EVENTFOLD_BROKER_CAPACITY - `from_env_custom_broker_capacity`
- [x] Test: invalid listen address returns Err - `from_env_invalid_listen_addr_returns_err`
- [x] Quality gates pass - All four gates pass clean

## Test Results
- Lint: PASS (`cargo clippy --all-targets --all-features --locked -- -D warnings`)
- Tests: PASS (183 total: 150 lib + 7 bin + 2 + 23 integration + 1 writer_integration + 0 doc-tests)
- Build: PASS (`cargo build` with zero warnings)
- Format: PASS (`cargo fmt --check`)
- New tests added: 7 tests in `src/main.rs` (`#[cfg(test)] mod tests`)
  - `from_env_defaults_when_only_data_set`
  - `from_env_missing_data_returns_err`
  - `from_env_custom_listen_addr`
  - `from_env_custom_broker_capacity`
  - `from_env_invalid_listen_addr_returns_err`
  - `from_env_invalid_broker_capacity_returns_err`
  - `init_tracing_does_not_panic`

## Concerns / Blockers
- Added `serial_test = "3"` as a dev-dependency, which was not explicitly listed in the ticket scope. This is required for correctness: Rust 2024 edition makes `std::env::set_var` and `std::env::remove_var` unsafe, and tests that mutate env vars must run serially to avoid data races. This is standard practice in the codebase's other projects.
- The `#[allow(dead_code)]` annotations should be removed by Ticket 2 when `Config`, `from_env()`, and `init_tracing()` are wired into `main()`.
