# Code Review: Ticket 1 -- Add tonic/prost Dependencies and Proto Schema

**Ticket:** 1 -- Add tonic/prost Dependencies and Proto Schema
**Impl Report:** prd/006-grpc-service-reports/ticket-01-impl.md
**Date:** 2026-02-26 16:45
**Verdict:** APPROVED

---

## AC Coverage

| AC # | Description | Status | Notes |
|------|-------------|--------|-------|
| 1 | `Cargo.toml` has `tonic = "0.13"` and `prost = "0.13"` in `[dependencies]` | Met | Lines 13, 16 of `Cargo.toml` |
| 2 | `Cargo.toml` has `tonic-build = "0.13"` in `[build-dependencies]` | Met | Line 26 of `Cargo.toml` |
| 3 | Proto uses `syntax = "proto3"` and `package eventfold` | Met | Lines 1-2 of `proto/eventfold.proto` |
| 4 | Proto defines `EventStore` service with 5 RPCs: `Append` (unary), `ReadStream` (unary), `ReadAll` (unary), `SubscribeAll` (server-streaming), `SubscribeStream` (server-streaming) | Met | Lines 4-10 of `proto/eventfold.proto`. All 5 RPCs present with correct signatures and `stream` keywords |
| 5 | Proto defines all message types matching PRD exactly: `ProposedEvent`, `RecordedEvent`, `ExpectedVersion` (with oneof kind), `Empty`, `AppendRequest`, `AppendResponse`, `ReadStreamRequest`, `ReadStreamResponse`, `ReadAllRequest`, `ReadAllResponse`, `SubscribeAllRequest`, `SubscribeStreamRequest`, `SubscribeResponse` (with oneof content) | Met | All 13 message types present with correct field names, types, and numbers. Field-by-field comparison against PRD section "Proto Definition" (lines 33-119 of PRD) confirms exact match |
| 6 | `build.rs` invokes `tonic_build::compile_protos("proto/eventfold.proto")` and returns `Ok(())` | Met | `build.rs` lines 1-4. Returns `Result<(), Box<dyn std::error::Error>>`, calls `compile_protos` with `?`, returns `Ok(())` |
| 7 | Generated types accessible; `cargo build` compiles without errors; module accessible via `tonic::include_proto!` | Met | `src/lib.rs` lines 7-9: `pub mod proto { tonic::include_proto!("eventfold"); }`. Build passes with zero warnings |
| 8 | Test: `AppendRequest::default()` constructs successfully in `#[cfg(test)]` block | Met | `src/lib.rs` lines 89-95: `proto_append_request_default` test verifies `AppendRequest::default()` and asserts `stream_id.is_empty()`, `events.is_empty()`, `expected_version.is_none()` |
| 9 | Quality gates pass: `cargo build`, `cargo clippy`, `cargo fmt --check`, `cargo test` | Met | Independently verified: build (zero warnings), clippy (zero diagnostics), fmt (passes), tests (165 total: 148 unit + 2 broker integration + 14 grpc integration + 1 writer integration, all green) |

## Issues Found

### Critical (must fix before merge)
- None

### Major (should fix, risk of downstream problems)
- None

### Minor (nice to fix, not blocking)
- None

## Suggestions (non-blocking)
- The `build.rs` file is minimal and correct. If additional proto files are added in the future, `tonic_build::configure()` builder with `.compile_protos()` would provide more flexibility (e.g., disabling client generation, file descriptor sets), but for now the simple form is appropriate.

## Scope Check
- Files within scope: YES. The four files specified in the ticket scope were the ones modified/created: `Cargo.toml`, `proto/eventfold.proto`, `build.rs`, `src/lib.rs`.
- Scope creep detected: MINOR (not blocking). The `src/lib.rs` diff also includes `pub mod service;` and `pub use service::EventfoldService;` (from Ticket 2/5), and `Cargo.toml` includes `tokio-stream = "0.1"` (a dev-dep likely from later tickets). These are from sibling tickets coexisting in the same uncommitted working tree -- an established pattern in this project. Not flagging as Critical.
- Unauthorized dependencies added: NO. `tonic = "0.13"`, `prost = "0.13"`, and `tonic-build = "0.13"` are all specified by the PRD and ticket scope. `tokio-stream` is from a sibling ticket.

## Risk Assessment
- Regression risk: LOW. This is purely additive: new dependencies, a new proto file, a new build script, and a new module declaration. No existing code was modified in a way that could break.
- Security concerns: NONE
- Performance concerns: NONE
