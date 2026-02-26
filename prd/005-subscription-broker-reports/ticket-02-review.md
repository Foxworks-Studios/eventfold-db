# Code Review: Ticket 2 -- Broker Struct with Publish, Subscribe, and Arc Sharing

**Ticket:** 2 -- Broker Struct with Publish, Subscribe, and Arc Sharing
**Impl Report:** prd/005-subscription-broker-reports/ticket-02-impl.md
**Date:** 2026-02-26 14:30
**Verdict:** APPROVED

---

## AC Coverage

| AC # | Description | Status | Notes |
|------|-------------|--------|-------|
| 1 | `Broker` is a `pub` struct with private field `tx: broadcast::Sender<Arc<RecordedEvent>>` | Met | `src/broker.rs:25-27` -- struct is `pub`, field `tx` has no `pub` modifier (private by default), type is exactly `broadcast::Sender<Arc<RecordedEvent>>` |
| 2 | `Broker::new(capacity: usize) -> Broker` constructs broadcast channel | Met | `src/broker.rs:36-41` -- calls `broadcast::channel(capacity)`, discards initial receiver, returns `Self { tx }` |
| 3 | `Broker::publish(&self, events: &[RecordedEvent])` wraps in Arc, sends, warns on no receivers | Met | `src/broker.rs:54-61` -- iterates events, wraps each in `Arc::new(event.clone())`, calls `self.tx.send(arc_event)`, logs `tracing::warn!` on send error |
| 4 | `Broker::subscribe(&self) -> broadcast::Receiver<Arc<RecordedEvent>>` | Met | `src/broker.rs:72-74` -- delegates to `self.tx.subscribe()` |
| 5 | Test AC-1: 3 events arrive at subscriber | Met | `publish_three_events_received_by_subscriber` (line 98-117) -- creates broker(16), subscribes, publishes 3 events, asserts all 3 arrive with correct `event_type` |
| 6 | Test AC-2: multiple subscribers each receive all events | Met | `multiple_subscribers_each_receive_all_events` (line 120-139) -- two subscribers, 2 events published, both receive both with correct types |
| 7 | Test AC-3: lagged subscriber gets `RecvError::Lagged` | Met | `lagged_subscriber_receives_lagged_error` (line 142-165) -- broker(2), publishes 5 without receiving, loops up to 6 times checking for `Lagged` error |
| 8 | Test AC-12: `Arc::ptr_eq` confirms shared allocation | Met | `arc_sharing_across_subscribers` (line 168-184) -- two subscribers, 1 event, asserts `Arc::ptr_eq` on received arcs |
| 9 | `Broker` accessible at crate root | Met | `src/lib.rs:3` adds `pub mod broker;`, line 11 adds `pub use broker::Broker;` |
| 10 | Quality gates pass | Met | Verified: `cargo build` (0 warnings), `cargo clippy --all-targets --all-features --locked -- -D warnings` (0 diagnostics), `cargo fmt --check` (clean), `cargo test` (120 unit + 1 integration = 121 total, 0 failures) |

## Issues Found

### Critical (must fix before merge)

None.

### Major (should fix, risk of downstream problems)

None.

### Minor (nice to fix, not blocking)

None.

## Suggestions (non-blocking)

- The `make_event` test helper in `broker::tests` creates events with `stream_id: Uuid::new_v4()` and `stream_version: 0`. This is fine for the broker tests (which do not care about stream semantics), but if later test modules need a similar helper, it could be extracted into a shared test utility module. No action needed now -- Ticket 3 and beyond will likely create their own helpers or reuse existing ones from `writer::tests`.

## Scope Check

- Files within scope: YES -- `src/broker.rs` (created) and `src/lib.rs` (modified) are the only files in Ticket 2's scope. Changes to `src/types.rs`, `Cargo.toml`, and `Cargo.lock` are from Ticket 1 (the prerequisite) and are co-mingled in the uncommitted working tree, which is the expected workflow for this project.
- Scope creep detected: NO
- Unauthorized dependencies added: NO -- `async-stream` was added by Ticket 1, not Ticket 2. Ticket 2 adds no new dependencies.

## Risk Assessment

- Regression risk: LOW -- `src/broker.rs` is a new file with no modifications to existing code. The only existing-file change is two additive lines in `src/lib.rs` (`pub mod broker;` and `pub use broker::Broker;`). All 121 tests pass, including the full pre-existing suite.
- Security concerns: NONE
- Performance concerns: NONE -- `publish` clones each `RecordedEvent` once into `Arc::new()`, which is the minimum required to satisfy the design requirement of shared allocations across subscribers. The broadcast channel capacity is caller-controlled.

## Implementation Quality Notes

The implementation is clean, minimal, and correct:

1. **Doc comments** are thorough on all public items (`Broker` struct, `new`, `publish`, `subscribe`), with `# Arguments`, `# Returns`, and `# Design` sections where appropriate.
2. **No `.unwrap()` in library code** -- the send error in `publish` is handled via `if let Err`, and tests use `.expect()` with descriptive messages.
3. **Test structure** follows Arrange-Act-Assert, each test maps clearly to a specific AC, and test names are descriptive.
4. **The lagged subscriber test** (AC-3) correctly handles the non-deterministic nature of broadcast channel lagging by looping and checking for the `Lagged` variant rather than asserting on a specific recv index.
5. **The `Arc::ptr_eq` test** (AC-12) correctly proves that the broadcast channel shares the same `Arc` allocation across subscribers, which is the key design goal from the CLAUDE.md "Broadcast channel cloning" landmine.
