# Implementation Report: Ticket 5 -- Refactor `eventfold-console` -- replace `ReadAll` scan with `ListStreams` RPC; remove `collect_streams`

**Ticket:** 5 - Refactor `eventfold-console` -- replace `ReadAll` scan with `ListStreams` RPC; remove `collect_streams`
**Date:** 2026-02-27 12:00
**Status:** COMPLETE

---

## Files Changed

### Created
- None

### Modified
- `eventfold-console/src/client.rs` - Replaced `list_streams` method: removed `page_size` parameter, replaced `ReadAll` scan loop with single `ListStreams` RPC call, added `ListStreamsRequest` to proto imports.
- `eventfold-console/src/app.rs` - Removed `collect_streams` method from `AppState`, removed its two tests (`collect_streams_groups_by_stream_id`, `collect_streams_empty_input_returns_empty`), removed `make_event_with_stream` test helper (only used by those tests), removed `HashMap` import (only used by `collect_streams`).
- `eventfold-console/src/main.rs` - Removed `LIST_STREAMS_PAGE_SIZE` constant, updated call site from `client.list_streams(LIST_STREAMS_PAGE_SIZE)` to `client.list_streams()`.

## Implementation Notes
- The `ReadAllRequest` import remains in `client.rs` because the `read_all` method still uses it. Only the `list_streams` method was changed; other callers of `ReadAllRequest` are unaffected.
- The `make_event_with_stream` test helper in `app.rs` was only used by the two `collect_streams` tests. It was removed to avoid dead code in the test module.
- The `HashMap` import in `app.rs` was only used by `collect_streams`. Removing it prevents an `unused_imports` warning.
- All changes were made atomically since `client.rs` called `AppState::collect_streams` -- removing the method without updating the caller would break compilation.
- The proto `StreamInfo.stream_id` is a `String`, and `app::StreamInfo.stream_id` is also a `String`, so the mapping is a direct field copy with no conversion needed.

## Acceptance Criteria
- [x] AC 1: `Client::list_streams(&mut self) -> Result<Vec<StreamInfo>, ConsoleError>` takes no `page_size` parameter; its body calls `self.inner.list_streams(ListStreamsRequest {}).await?.into_inner()` and maps each `proto::StreamInfo` to `app::StreamInfo`.
- [x] AC 2: The imports at the top of `client.rs` reference `ListStreamsRequest` from `eventfold_db::proto`; `ReadAllRequest` remains because `read_all` still uses it.
- [x] AC 3: `AppState::collect_streams` is entirely absent from `app.rs` with no remaining `collect_streams` references in the file.
- [x] AC 4: `LIST_STREAMS_PAGE_SIZE` constant is absent from `main.rs`; the call site reads `client.list_streams().await`.
- [x] AC 5: `cargo grep collect_streams` returns zero matches across the workspace (verified with Grep tool).
- [x] AC 6: Quality gates pass -- `cargo build`, `cargo clippy --all-targets --all-features --locked -- -D warnings`, `cargo test`, `cargo fmt --check` all clean.

## Test Results
- Lint: PASS (clippy clean, zero warnings)
- Tests: PASS (291 tests, 0 failures -- 2 tests removed: `collect_streams_groups_by_stream_id`, `collect_streams_empty_input_returns_empty`)
- Build: PASS (zero warnings)
- Format: PASS
- New tests added: None (this ticket removes dead code; no new behavior to test)

## Concerns / Blockers
- None
