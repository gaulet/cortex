//! # cortex-mcp-server
//!
//! Main binary for Cortex MCP server.
//!
//! This binary exposes 7 + 1 bonus tools via stdio-based MCP:
//! - `get_routing_rules` : Boot handshake (bonus)
//! - `intercept_plan` : Generate fractal plan
//! - `approve_and_execute` : Launch workers (Fire-and-Forget)
//! - `sync_reflect` : Validate worker output
//! - `check_jobs_status` : Query project status
//! - `harvest_insights` : Extract patterns + lessons
//! - `rollback` : Restore to previous commit
//! - `abort` : Emergency stop
//!
//! ## Transport
//!
//! JSON-RPC 2.0 over stdio (stdio is the only supported transport).

use anyhow::Result;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

mod config;
mod tools;

/// Server entry point.
///
/// Boot sequence:
/// 1. Load config from environment / config file
/// 2. Initialize tracing (to stderr so MCP stdio stays clean)
/// 3. Connect to SQLite database and run migrations
/// 4. Start ActorRegistry with all required actors
/// 5. Start MCP stdio transport and handle requests until shutdown
#[tokio::main]
async fn main() -> Result<()> {
    // Tracing to stderr (MCP stdio uses stdout for JSON-RPC)
    let _subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .with_writer(std::io::stderr)
        .init();

    info!("Cortex MCP server starting");
    info!("Version: {}", env!("CARGO_PKG_VERSION"));

    // TODO: Load config
    // TODO: Initialize SQLite + WAL
    // TODO: Actor registry
    // TODO: MCP server stdio loop

    info!("Cortex MCP server ready (TODO: real init not yet done)");

    // Keep server alive (placeholder)
    tokio::signal::ctrl_c().await?;
    info!("Cortex MCP server shutting down");

    Ok(())
}
