//! Crow Telemetry - Trace capture for agent sessions
//!
//! This crate provides the core types and database for capturing traces from
//! agent sessions, including both native Zed agents and external agents like
//! Claude Code running via ACP.

mod db;
mod trace;

pub use db::*;
pub use trace::*;
