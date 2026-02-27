//! EventfoldDB interactive TUI console library.
//!
//! This crate provides the core components for the `eventfold-console` binary:
//! gRPC client, application state, TUI rendering, and view modules.

pub mod app;
pub mod client;
pub mod error;
pub mod tui;
pub mod views;
