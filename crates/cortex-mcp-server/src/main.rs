//! # cortex-mcp-server
//!
//! Serveur MCP (Model Context Protocol) pour Cortex.
//!
//! Expose 7 outils MCP + 1 bonus (`get_routing_rules`) via stdio.
//!
//! ## Architecture
//!
//! - `CortexServer<C: LlmClient>` : cœur métier (server.rs)
//! - Main : boucle stdio qui lit les requêtes JSON-RPC et les route vers CortexServer
//!
//! ## Transport
//!
//! JSON-RPC 2.0 over stdio (pas d'autre transport supporté).
//! Les logs sont envoyés sur stderr pour ne pas polluer la communication MCP.

pub mod server;

pub use server::{CortexServer, CortexServerError};

mod config;

use anyhow::Result;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

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
