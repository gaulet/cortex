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
//! - `metrics` : Lock-free Prometheus counters

pub mod context;
pub mod error;
pub mod graph;
pub mod metrics;
pub mod routing;
pub mod scratchpad;
pub mod wal;

pub use context::*;
pub use error::*;
pub use metrics::{Metrics, SharedMetrics};
pub use routing::RoutingRules;
pub use scratchpad::*;
pub use wal::{CortexCommit, RecoveryAction, RecoveryReport, WalEntry, WalService};

/// Génère un nouvel ID de projet unique.
pub fn generate_project_id() -> String {
    format!("project-{}", uuid::Uuid::now_v7())
}
