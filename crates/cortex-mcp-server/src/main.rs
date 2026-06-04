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
//!
//! ## Configuration via env vars
//!
//! - `CORTEX_WAL_URL` (default : `sqlite::memory:`) — URL du SQLite WAL
//! - `CORTEX_LLM_PROVIDER` (default : `mock`) — `mock` | `openai`
//! - `CORTEX_LLM_API_KEY` — API key pour OpenAI-compatible (obligatoire si provider=openai)
//! - `CORTEX_LLM_BASE_URL` (default : `https://openrouter.ai/api/v1`)
//! - `CORTEX_LLM_MODEL` (default : `minimax/minimax-m3`)
//! - `CORTEX_LLM_TIMEOUT` (default : 60) — timeout HTTP en secondes
//! - `CORTEX_LLM_REASONING` (default : `max`) — effort de raisonnement : `low` | `medium` | `high` | `max`
//! - `CORTEX_HMAC_SECRET` (optionnel) — secret HMAC-SHA256 pour signer les guardrails Pre-Mortem
//!
//! Exemple pour utiliser MiniMax M3 via OpenRouter avec raisonnement max :
//! ```bash
//! CORTEX_LLM_PROVIDER=openai \
//! CORTEX_LLM_API_KEY=*** \
//! CORTEX_LLM_BASE_URL=https://openrouter.ai/api/v1 \
//! CORTEX_LLM_MODEL=minimax/minimax-m3 \
//! CORTEX_LLM_REASONING=max \
//! cargo run --bin cortex-mcp
//! ```

mod config;
mod dispatch;
mod protocol;
pub mod server;

pub use server::{CortexServer, CortexServerError};

use std::io::{self, BufRead, Write};

use async_trait::async_trait;
use cortex_brains::{
    Architect, HttpLlmClient, LlmClient, LlmError, LlmRequest, LlmResponse, MockLlmClient,
};
use cortex_core::WalService;

use dispatch::dispatch;
use protocol::{JsonRpcErrorResponse, JsonRpcRequest, JsonRpcResponse, PARSE_ERROR};

use anyhow::{Context, Result};
use tracing::{error, info, warn, Level};
use tracing_subscriber::FmtSubscriber;

/// LLM client concret (mock ou http), Clone + Send + Sync.
///
/// Permet d'utiliser `CortexServer<AnyLlmClient>` qui satisfait
/// `C: LlmClient + Clone` sans utiliser `Box<dyn>`.
#[derive(Clone)]
enum AnyLlmClient {
    Mock(MockLlmClient),
    Http(HttpLlmClient),
}

#[async_trait]
impl LlmClient for AnyLlmClient {
    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        match self {
            AnyLlmClient::Mock(c) => c.complete(request).await,
            AnyLlmClient::Http(c) => c.complete(request).await,
        }
    }
}

/// Point d'entrée du serveur MCP.
#[tokio::main]
async fn main() -> Result<()> {
    // 1. Tracing vers stderr
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .with_writer(std::io::stderr)
        .finish();
    tracing::subscriber::set_global_default(subscriber).expect("failed to set tracing subscriber");

    info!("Cortex MCP server starting");
    info!("Version: {}", env!("CARGO_PKG_VERSION"));

    // 2. WAL service
    let wal_url = std::env::var("CORTEX_WAL_URL").unwrap_or_else(|_| "sqlite::memory:".to_string());
    let wal = WalService::connect(&wal_url)
        .await
        .with_context(|| format!("Failed to connect WAL at {}", wal_url))?;
    info!("WAL service initialized (url={})", wal_url);

    // 3. LLM client
    let llm = build_llm_client().context("Failed to build LLM client")?;
    let architect = Architect::new(llm.clone());
    let server = CortexServer::new(wal, architect, llm)
        .with_hmac_secret(std::env::var("CORTEX_HMAC_SECRET").ok().as_deref());
    if server.hmac_secret.is_some() {
        info!("HMAC signing enabled (guardrails will be signed)");
    } else {
        info!("HMAC signing disabled (set CORTEX_HMAC_SECRET to enable)");
    }
    info!("Ready. Entering stdio loop (Ctrl+C to exit).");

    // 4. Stdio loop
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

        let parsed = JsonRpcRequest::parse_line(&line);
        let response = match parsed {
            Err(err) => Some(JsonRpcErrorResponse::error(None, PARSE_ERROR, err.message).to_line()),
            Ok(None) => None,
            Ok(Some(req)) if req.is_notification() => {
                info!("Received notification: {}", req.method);
                dispatch(&server, &req.method, req.params).await;
                None
            }
            Ok(Some(req)) => {
                info!("Received request: {} (id={:?})", req.method, req.id);
                match dispatch(&server, &req.method, req.params).await {
                    Some(Ok(result)) => {
                        // req.id est Option<JsonValue> : None = notification JSON-RPC (pas de réponse).
                        // On ne panic plus : si id est None, on ne répond pas.
                        req.id
                            .map(|id| JsonRpcResponse::success(id, result).to_line())
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

/// Construit le LLM client selon la configuration env.
///
/// Variantes :
/// - `mock` (default) : MockLlmClient avec plan placeholder
/// - `openai` : HttpLlmClient OpenAI-compatible (OpenRouter, OpenAI, ollama, vLLM)
fn build_llm_client() -> Result<AnyLlmClient> {
    let provider = std::env::var("CORTEX_LLM_PROVIDER").unwrap_or_else(|_| "mock".to_string());

    match provider.as_str() {
        "mock" => {
            info!("Using MockLlmClient (CORTEX_LLM_PROVIDER=mock)");
            let mock_plan = r#"{"themes":[{"id":"TH-1","name":"Plan placeholder (mock LLM)","is_parallel_branch":false,"convergence_contract":null,"depends_on":[],"criticity_score":2,"resources_used":[],"concurrency_group":null,"tasks":[{"id":"T-1.1","name":"TODO: brancher vrai LLM","definition_of_done":"LLM reel repond","depends_on":[]}]}],"concurrency_groups":[],"parking_lot":[],"ignored_noise":[],"impact_warnings":[]}"#;
            Ok(AnyLlmClient::Mock(MockLlmClient::with_response(
                mock_plan.to_string(),
            )))
        }
        "openai" => {
            let api_key = std::env::var("CORTEX_LLM_API_KEY")
                .context("CORTEX_LLM_API_KEY required for provider=openai")?;
            let base_url = std::env::var("CORTEX_LLM_BASE_URL")
                .unwrap_or_else(|_| "https://openrouter.ai/api/v1".to_string());
            let model = std::env::var("CORTEX_LLM_MODEL")
                .unwrap_or_else(|_| "minimax/minimax-m3".to_string());
            let timeout: u64 = std::env::var("CORTEX_LLM_TIMEOUT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(60);
            let reasoning = std::env::var("CORTEX_LLM_REASONING").ok();

            info!(
                "Using HttpLlmClient: base_url={} model={} timeout={}s reasoning={:?}",
                base_url, model, timeout, reasoning
            );

            let client = HttpLlmClient::new(api_key, base_url, model)
                .with_timeout(timeout)
                .with_reasoning(reasoning.as_deref());
            Ok(AnyLlmClient::Http(client))
        }
        other => Err(anyhow::anyhow!(
            "Unknown CORTEX_LLM_PROVIDER '{}'. Valid: mock, openai",
            other
        )),
    }
}
