# Implementation Report: Ticket 3 -- Add TlsOptions struct and update Client::connect in eventfold-console

**Ticket:** 3 - Add TlsOptions struct and update Client::connect in eventfold-console
**Date:** 2026-02-27 12:00
**Status:** COMPLETE

---

## Files Changed

### Created
- None

### Modified
- `eventfold-console/src/client.rs` - Added `TlsOptions` struct with `Debug, Clone, Default` derives and fields `enabled`, `ca_pem`, `identity`. Updated `Client::connect` signature to accept `TlsOptions` as second parameter. Implemented TLS channel construction branch (using `ClientTlsConfig`, `Certificate`, `Identity`, and `Channel::from_shared`) when `tls.enabled` is true. Fixed pre-existing clippy lint (`or_else` -> `inspect_err` in `spawn_subscription`). Added 3 new tests.
- `eventfold-console/src/main.rs` - Updated `Client::connect(&cli.addr)` call to `Client::connect(&cli.addr, TlsOptions::default())`. Added `TlsOptions` import. Fixed pre-existing clippy lint (collapsible `if` in `run_event_loop`).
- `eventfold-console/src/app.rs` - Fixed pre-existing clippy lint (collapsible `if` in `apply_action` for `Action::Select`). Out of ticket scope but required for quality gate.
- `eventfold-console/src/views/mod.rs` - Fixed pre-existing clippy lint (collapsible `if` in `format_bytes`). Out of ticket scope but required for quality gate.

## Implementation Notes
- The TLS branch uses `Channel::from_shared(addr)?.tls_config(tls_config)?.connect().await` as specified in the ticket and PRD.
- The plaintext branch was changed from `EventStoreClient::connect(addr)` to `Channel::from_shared(addr)?.connect().await` + `EventStoreClient::new(channel)` for consistency -- both paths now use `Channel::from_shared` which normalizes error handling.
- All connection errors (invalid URI, TLS config errors, transport errors) are mapped to `ConsoleError::ConnectionFailed` with the original error message preserved.
- Four pre-existing clippy lints in the console crate were fixed as a side effect (all were `collapsible_if` or `bind_instead_of_map`/`manual_inspect` lints from Rust 2024 edition's let-chains). These are trivial, behavior-preserving changes required to pass the `-D warnings` quality gate.

## Acceptance Criteria
- [x] AC 1: `TlsOptions` has fields `enabled: bool`, `ca_pem: Option<Vec<u8>>`, `identity: Option<(Vec<u8>, Vec<u8>)>`. It derives `Debug`, `Clone`, `Default`. -- Struct defined at line 28-36 of `client.rs`.
- [x] AC 2: `Client::connect` signature is `pub async fn connect(addr: &str, tls: TlsOptions) -> Result<Self, ConsoleError>`. -- Line 65 of `client.rs`.
- [x] AC 3: Test: `TlsOptions::default()` has `enabled = false`, `ca_pem = None`, `identity = None`. -- Test `tls_options_default_has_tls_disabled` at line 365.
- [x] AC 4: Test: `Client::connect("http://[::1]:0", TlsOptions::default()).await` returns `Err(ConsoleError::ConnectionFailed(_))`. -- Test `connect_plaintext_no_server_returns_connection_failed` at line 373.
- [x] AC 5: Test: `Client::connect("http://[::1]:0", TlsOptions { enabled: true, ..Default::default() }).await` returns `Err(ConsoleError::ConnectionFailed(_))`. -- Test `connect_tls_no_server_returns_connection_failed` at line 384.
- [x] AC 6: Existing `proto_to_event_record`, `client_is_debug`, and `client_is_clone` unit tests still pass without modification. -- All pass unchanged.
- [x] AC 7: `eventfold-console/src/main.rs` passes `TlsOptions::default()` so the console compiles and the default (no-TLS) behavior is unchanged. -- Line 55 of `main.rs`.
- [x] AC 8: Quality gates pass (build, lint, fmt, tests). -- All pass for `eventfold-console` crate. Root crate has pre-existing failures (fmt issue in `src/main.rs`, clippy `single_component_path_imports` lint, one failing test `binary_exits_nonzero_when_tls_cert_file_missing`) -- none introduced by this ticket.

## Test Results
- Lint (console): PASS (`cargo clippy -p eventfold-console --all-targets --all-features --locked -- -D warnings`)
- Tests (console): PASS (63 tests, 0 failures)
- Build (workspace): PASS (zero warnings)
- Fmt (console): PASS (`cargo fmt -p eventfold-console --check`)
- New tests added:
  - `eventfold-console/src/client.rs::tests::tls_options_default_has_tls_disabled`
  - `eventfold-console/src/client.rs::tests::connect_plaintext_no_server_returns_connection_failed`
  - `eventfold-console/src/client.rs::tests::connect_tls_no_server_returns_connection_failed`

## Concerns / Blockers
- Pre-existing quality issues in the root crate (`src/main.rs`): a `cargo fmt` diff, a clippy `single_component_path_imports` lint, and one failing test (`binary_exits_nonzero_when_tls_cert_file_missing`). These are not caused by this ticket and exist on the `main` branch.
- Two files outside the ticket's stated scope were modified (`app.rs`, `views/mod.rs`) to fix pre-existing clippy lints that blocked the `-D warnings` quality gate. These are trivial, behavior-preserving changes (nested `if` collapsed to `let`-chains per Rust 2024 edition).
