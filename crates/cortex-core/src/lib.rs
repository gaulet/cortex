//! # cortex-core
//!
//! Core domain model for Cortex MCP server.
//!
//! Contains data structures (Scratchpad, Theme, Job, etc.) and the WAL persistence layer.
//!
//! ## Architecture
//!
//! - `scratchpad` : Project scratchpad with themes, jobs, milestones
//! - `graph` : Dependency graph between themes
//! - `wal` : Write-Ahead Logging for crash recovery
//! - `context` : Context packets exchanged with Hermes
//! - `error` : Core error types

pub mod context;
pub mod error;
pub mod graph;
pub mod routing;
pub mod scratchpad;
pub mod wal;

pub use context::*;
pub use error::*;
pub use routing::RoutingRules;
pub use scratchpad::*;
