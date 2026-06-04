//! # cortex-mcp-server
//!
//! Serveur MCP (Model Context Protocol) pour Cortex.
//!
//! Expose 7 outils MCP + 1 bonus (`get_routing_rules`) via stdio.
//!
//! ## Architecture
//!
//! - `CortexServer<C: LlmClient>` : cœur métier (server.rs)
//! - `protocol` : parser/formatter JSON-RPC 2.0
//! - `dispatch` : route les calls vers CortexServer
//! - Main : boucle stdin → parse → dispatch → output stdout
//!
//! ## Transport
//!
//! JSON-RPC 2.0 over stdio (pas d'autre transport supporté).
//! Les logs sont envoyés sur stderr pour ne pas polluer la communication MCP.

mod config;
mod dispatch;
mod protocol;
pub mod server;

pub use server::{CortexServer, CortexServerError};

use std::io::{self, BufRead, Write};

use cortex_brains::{Architect, MockLlmClient};
use cortex_core::WalService;

use dispatch::dispatch;
use protocol::{JsonRpcErrorResponse, JsonRpcRequest, JsonRpcResponse, PARSE_ERROR};

use anyhow::Result;
use tracing::{error, info, warn, Level};
use tracing_subscriber::FmtSubscriber;

/// Point d'entrée du serveur MCP.
///
/// ## Boot sequence
///
/// 1. Initialize tracing vers stderr
/// 2. Connect to SQLite (WAL) database
/// 3. Create CortexServer<MockLlmClient> (MVP)
/// 4. Enter stdio loop: read JSON-RPC → dispatch → write response
/// 5. Exit on stdin EOF or SIGINT
#[tokio::main]
async fn main() -> Result<()> {
    // 1. Tracing vers stderr (sinon pollue le flux MPI stdout)
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .with_writer(std::io::stderr)
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .expect("failed to set tracing subscriber");

    info!("Cortex MCP server starting");
    info!("Version: {}", env!("CARGO_PKG_VERSION"));

    // 2. Connect to SQLite in-memory (MVP). TODO: file-based via config
    let wal = WalService::connect("sqlite::memory:")
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect WAL: {}", e))?;
    info!("WAL service initialized (in-memory)");

    // 3. Create CortexServer with mock LLM (MVP). TODO: real LLM client via config
    let mock_plan = r#"{"themes":[{"id":"TH-1","name":"Plan placeholder (mock LLM)","is_parallel_branch":false,"convergence_contract":null,"depends_on":[],"criticity_score":2,"resources_used":[],"concurrency_group":null,"tasks":[{"id":"T-1.1","name":"TODO: brancher vrai LLM","definition_of_done":"LLM reel repond","depends_on":[]}]}],"concurrency_groups":[],"parking_lot":[],"ignored_noise":[],"impact_warnings":[]}"#;
    let mock_llm = MockLlmClient::with_response(mock_plan.to_string());
    let architect = Architect::new(mock_llm);
    let server = CortexServer::new(wal, architect);
    info!("CortexServer initialized with MockLlmClient (MVP)");

    info!("Ready. Entering stdio loop (Ctrl+C to exit).");

    // 4. Stdio loop : read line from stdin, dispatch, write response to stdout
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdout_handle = stdout.lock();

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                error!("Failed to read stdin: {}", e);
                break;
            }
        };

        // Parse the line
        let parsed = JsonRpcRequest::parse_line(&line);

        let response = match parsed {
            // JSON invalid → error response with null id
            Err(err) => Some(JsonRpcErrorResponse::error(None, PARSE_ERROR, err.message).to_line()),

            // Empty line/whitespace → ignore
            Ok(None) => None,

            // Valid notification (no id) → call dispatch but no output
            Ok(Some(req)) if req.is_notification() => {
                info!("Received notification: {}", req.method);
                dispatch(&server, &req.method, req.params).await;
                None
            }

            // Valid request with id → dispatch and output response
            Ok(Some(req)) => {
                info!("Received request: {} (id={:?})", req.method, req.id);
                match dispatch(&server, &req.method, req.params).await {
                    Some(Ok(result)) => {
                        Some(JsonRpcResponse::success(req.id.unwrap(), result).to_line())
                    }
                    Some(Err(err)) => {
                        Some(JsonRpcErrorResponse::error(req.id, err.code, err.message).to_line())
                    }
                    None => {
                        warn!("Dispatch returned None for request with id (should not happen)");
                        None
                    }
                }
            }
        };

        if let Some(line) = response {
            stdout_handle.write_all(line.as_bytes())?;
            stdout_handle.flush()?;
        }
    }

    info!("Stdin closed. Shutting down.");
    Ok(())
}
