# Tickets for PRD 001: Core Types and Error Handling

**Source PRD:** prd/001-core-types-and-errors.md
**Created:** 2026-02-25
**Total Tickets:** 4
**Estimated Total Complexity:** 8 (S=1, M=2, L=3 → S + M + M + M = 1 + 2 + 2 + 2 = 7... see below)

> Complexity sum: Ticket 1 = M(2), Ticket 2 = M(2), Ticket 3 = S(1), Ticket 4 = M(2) → **Total: 7**

---

### Ticket 1: Add Cargo.toml dependencies and implement `types.rs`

**Description:**
Add the three required dependencies (`bytes`, `thiserror`, `uuid`) to `Cargo.toml`, then create
`src/types.rs` with `ProposedEvent`, `RecordedEvent`, `ExpectedVersion`, and the two constants
(`MAX_EVENT_SIZE`, `MAX_EVENT_TYPE_LEN`). All public items must have doc comments. Write a
`#[cfg(test)]` module inside `types.rs` covering AC-1 through AC-4 using red/green TDD (write
each failing test before its implementation).

**Scope:**
- Modify: `Cargo.toml` (add `bytes = "1"`, `thiserror = "2"`, `uuid = { version = "1", features = ["v4", "v7"] }`)
- Create: `src/types.rs`

**Acceptance Criteria:**
- [ ] `Cargo.toml` contains `bytes`, `thiserror`, and `uuid` dependencies at the specified versions with the `v4`/`v7` features enabled on `uuid`.
- [ ] `ProposedEvent` has fields `event_id: Uuid`, `event_type: String`, `metadata: Bytes`, `payload: Bytes`; derives `Debug`, `Clone`, `PartialEq`.
- [ ] `RecordedEvent` has all seven fields as specified in the PRD; derives `Debug`, `Clone`, `PartialEq`.
- [ ] `ExpectedVersion` enum has variants `Any`, `NoStream`, and `Exact(u64)`; derives `Debug`, `Clone`, `Copy`, `PartialEq`, `Eq`.
- [ ] `MAX_EVENT_SIZE` equals `65536` (64 * 1024) and `MAX_EVENT_TYPE_LEN` equals `256`; both are `pub const usize`.
- [ ] Test: create a `ProposedEvent`, assert all fields round-trip correctly, and assert a clone is equal to the original (AC-1).
- [ ] Test: create a `RecordedEvent`, assert all fields, assert clone equality, and assert two events differing only in `global_position` are not equal (AC-2).
- [ ] Test: `ExpectedVersion::Any` is `Copy` (bind to a variable, use it twice without `clone()`); `NoStream` constructs; `Exact(5)` pattern-matches to extract `5`; `Debug` format is non-empty; `Exact(3) == Exact(3)` and `Exact(3) != Exact(4)` (AC-3).
- [ ] Test: `MAX_EVENT_SIZE == 65536` and `MAX_EVENT_TYPE_LEN == 256` (AC-4).
- [ ] `cargo build` emits zero warnings; `cargo clippy -- -D warnings` passes; `cargo fmt --check` passes; `cargo test` is all green.

**Dependencies:** None
**Complexity:** M
**Maps to PRD AC:** AC-1, AC-2, AC-3, AC-4

---

### Ticket 2: Implement `error.rs` with all error variants and tests

**Description:**
Create `src/error.rs` containing the `Error` enum with all seven variants as specified in the PRD,
derived with `thiserror::Error` and `Debug`. Write a `#[cfg(test)]` module inside `error.rs`
covering every AC-5 test case using red/green TDD. The module must be compilable in isolation
(it uses only `std::io::Error` and `uuid::Uuid` from dependencies already added in Ticket 1).

**Scope:**
- Create: `src/error.rs`

**Acceptance Criteria:**
- [ ] `Error` enum has exactly seven variants: `WrongExpectedVersion { expected: String, actual: String }`, `StreamNotFound { stream_id: Uuid }`, `Io(#[from] std::io::Error)`, `CorruptRecord { position: u64, detail: String }`, `InvalidHeader(String)`, `EventTooLarge { size: usize, max: usize }`, `InvalidArgument(String)`.
- [ ] `#[derive(Debug, thiserror::Error)]` is applied to `Error`; the `thiserror` `#[error("...")]` format strings match the PRD specification exactly.
- [ ] Test: `Error::WrongExpectedVersion { expected: "0".into(), actual: "1".into() }` — `to_string()` contains both "wrong expected version" and the expected/actual values (AC-5).
- [ ] Test: `Error::StreamNotFound { stream_id }` — `to_string()` contains the UUID string representation (AC-5).
- [ ] Test: `std::io::Error::new(std::io::ErrorKind::NotFound, "file missing")` converts via `Error::from()`/`?`-coercion into `Error::Io`; `to_string()` contains "I/O error" (AC-5).
- [ ] Test: `Error::CorruptRecord { position: 42, detail: "bad crc".into() }` — `to_string()` contains `"42"` and `"bad crc"` (AC-5).
- [ ] Test: `Error::InvalidHeader("bad magic".into())` — `to_string()` contains `"bad magic"` (AC-5).
- [ ] Test: `Error::EventTooLarge { size: 70000, max: 65536 }` — `to_string()` contains `"70000"` and `"65536"` (AC-5).
- [ ] Test: `Error::InvalidArgument("stream_id is empty".into())` — `to_string()` contains `"stream_id is empty"` (AC-5).
- [ ] Test: all seven variants format non-empty strings via `{:?}` (AC-5 Debug requirement).
- [ ] `cargo build` emits zero warnings; `cargo clippy -- -D warnings` passes; `cargo fmt --check` passes; `cargo test` is all green.

**Dependencies:** Ticket 1 (requires `uuid` and `thiserror` in `Cargo.toml`)
**Complexity:** M
**Maps to PRD AC:** AC-5

---

### Ticket 3: Implement `src/lib.rs` re-exports

**Description:**
Replace the stub `main.rs`-only crate root with a proper `lib.rs` that declares `pub mod error`
and `pub mod types`, then re-exports `Error`, and all public items from `types` via `pub use`.
This makes the public API surface (`eventfold_db::RecordedEvent`, etc.) accessible to downstream
crates and integration tests. Write a `#[cfg(test)]` module in `lib.rs` that imports each
re-exported symbol by its crate-root path to confirm the exports compile and resolve (AC-6).

**Scope:**
- Create: `src/lib.rs`

**Acceptance Criteria:**
- [ ] `src/lib.rs` declares `pub mod error` and `pub mod types`.
- [ ] `pub use error::Error` makes `eventfold_db::Error` accessible.
- [ ] `pub use types::*` (or explicit re-exports) makes `eventfold_db::RecordedEvent`, `eventfold_db::ProposedEvent`, `eventfold_db::ExpectedVersion`, `eventfold_db::MAX_EVENT_SIZE`, and `eventfold_db::MAX_EVENT_TYPE_LEN` accessible at the crate root.
- [ ] Test inside `lib.rs`: use `crate::RecordedEvent`, `crate::ProposedEvent`, `crate::ExpectedVersion`, `crate::MAX_EVENT_SIZE`, `crate::MAX_EVENT_TYPE_LEN`, and `crate::Error` — each is constructible/usable, confirming re-exports resolve (AC-6).
- [ ] `cargo build` emits zero warnings; `cargo clippy -- -D warnings` passes; `cargo fmt --check` passes; `cargo test` is all green.

**Dependencies:** Ticket 1, Ticket 2
**Complexity:** S
**Maps to PRD AC:** AC-6

---

### Ticket 4: Verification and integration check

**Description:**
Run the full PRD 001 acceptance criteria checklist end-to-end. Verify all three modules compile
together, all unit tests pass, linting is clean, and the public API surface is exactly as
specified. This ticket requires no code changes if all prior tickets were implemented correctly;
its purpose is a final green-light gate before downstream PRDs build on these types.

**Scope:**
- Modify: none expected (fix any issues found during verification)

**Acceptance Criteria:**
- [ ] `cargo build 2>&1 | grep -c "^error" || true` outputs `0` — zero compile errors.
- [ ] `cargo build 2>&1 | grep -c "^warning" || true` outputs `0` — zero compiler warnings.
- [ ] `cargo clippy --all-targets --all-features --locked -- -D warnings` exits 0 with no diagnostics.
- [ ] `cargo fmt --check` exits 0.
- [ ] `cargo test` reports all tests passing (0 failures, 0 errors).
- [ ] All PRD acceptance criteria AC-1 through AC-6 are covered by passing tests (confirmed by test names visible in `cargo test -- --list` output).
- [ ] No regressions in any existing tests.

**Dependencies:** Ticket 1, Ticket 2, Ticket 3
**Complexity:** M
**Maps to PRD AC:** AC-7

---

## AC Coverage Matrix

| PRD AC # | Description                                                         | Covered By Ticket(s) | Status  |
|----------|---------------------------------------------------------------------|----------------------|---------|
| AC-1     | `ProposedEvent` construction, field access, and clone equality      | Ticket 1             | Covered |
| AC-2     | `RecordedEvent` construction, field access, clone equality, inequality | Ticket 1          | Covered |
| AC-3     | `ExpectedVersion` variants: Copy, constructibility, pattern match, Debug, equality | Ticket 1 | Covered |
| AC-4     | `MAX_EVENT_SIZE == 65536` and `MAX_EVENT_TYPE_LEN == 256`           | Ticket 1             | Covered |
| AC-5     | All seven `Error` variants: display strings, `From<io::Error>`, Debug | Ticket 2           | Covered |
| AC-6     | Crate re-exports accessible at `eventfold_db::*`                    | Ticket 3             | Covered |
| AC-7     | `cargo build` zero warnings, clippy clean, fmt clean, tests green   | Ticket 4             | Covered |
