//! Dispatch : route les appels MCP vers CortexServer.
//!
//! # Outils MCP supportés
//!
//! - `get_routing_rules` : Retourne les règles de routage Hermes→Cortex
//! - `intercept_plan` : Génère un plan fractal pour une intention
//! - `ping` : Retourne "ok"
//!
//! # Méthodes MCP supportées
//!
//! - `initialize` : Handshake retourne server capabilities
//! - `tools/list` : Liste les outils disponibles
//! - `tools/call` : Dispatch vers get_routing_rules / intercept_plan
//! - `ping` : Keep-alive

use cortex_brains::LlmClient;
use serde_json::{json, Value as JsonValue};

use crate::protocol::{
    JsonRpcError, JsonRpcResponse, INVALID_PARAMS, INVALID_REQUEST, METHOD_NOT_FOUND, PARSE_ERROR,
};
use crate::server::{CortexServer, InterceptPlanRequest};

// ============================================================================
// Dispatch result
// ============================================================================

/// Résultat du dispatch : succès (JsonRpcResponse) ou erreur (JsonRpcError).
pub type DispatchResult = Result<JsonValue, JsonRpcError>;

// ============================================================================
// Dispatch principal
// ============================================================================

/// Dispatch une méthode JSON-RPC vers CortexServer.
///
/// Retourne `Some(DispatchResult)` si la méthode doit renvoyer une réponse,
/// ou `None` si c'est une notification (silence).
pub async fn dispatch<C: LlmClient + Clone>(
    server: &CortexServer<C>,
    method: &str,
    params: Option<JsonValue>,
) -> Option<DispatchResult> {
    match method {
        "initialize" => Some(handle_initialize()),
        "ping" => Some(handle_ping()),
        "tools/list" => Some(handle_tools_list()),
        "tools/call" => Some(handle_tools_call(server, params).await),

        // Notifications: pas de réponse
        "notifications/initialized" | "notifications/cancelled" => None,

        // Méthode inconnue
        _ => Some(Err(JsonRpcError {
            code: METHOD_NOT_FOUND,
            message: format!("Method not found: {}", method),
            data: None,
        })),
    }
}

// ============================================================================
// Handlers de méthodes
// ============================================================================

/// Handshake initialize : retourne capabilities server.
fn handle_initialize() -> DispatchResult {
    Ok(json!({
        "protocolVersion": "2024-11-05",
        "serverInfo": {
            "name": "cortex-mcp",
            "version": env!("CARGO_PKG_VERSION")
        },
        "capabilities": {
            "tools": {
                "listChanged": false
            }
        }
    }))
}

/// Ping : keep-alive simple.
fn handle_ping() -> DispatchResult {
    Ok(json!({"status": "ok"}))
}

/// Liste des outils MCP disponibles.
fn handle_tools_list() -> DispatchResult {
    let tools = vec![
        json!({
            "name": "get_routing_rules",
            "description": "Returns Hermes→Cortex routing rules for boot handshake. Hermes uses these patterns to determine when Cortex should be invoked.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }),
        json!({
            "name": "intercept_plan",
            "description": "Generates a fractal execution plan (MECE decomposition) for a user intent. Uses the Architect LLM brain to decompose the task into themes, jobs, and dependencies.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "intent": {"type": "string", "description": "Raw user intent (free text)"},
                    "context": {"type": "string", "description": "Additional context (constraints, history, etc.)"},
                    "project_id": {"type": "string", "description": "Optional. Existing project ID, or omit to create new project."}
                },
                "required": ["intent"]
            }
        }),
        json!({
            "name": "pre_mortem",
            "description": "Generates executable guardrails for a high-criticality job (criticity ≥ 4). Uses the Pre-Mortem brain to anticipate F1-F19 failure modes.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "job_id": {"type": "string", "description": "ID of the job"},
                    "job_description": {"type": "string", "description": "What the job should do"},
                    "definition_of_done": {"type": "string", "description": "Binary verifiable success criteria"},
                    "context": {"type": "string", "description": "Environment, constraints"}
                },
                "required": ["job_id", "job_description", "definition_of_done"]
            }
        }),
        json!({
            "name": "red_team_audit",
            "description": "Adversarial audit of a worker-produced artifact across 5 layers (HMAC, DoD, guardrails, convergence, edge cases).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "job_id": {"type": "string"},
                    "definition_of_done": {"type": "string"},
                    "convergence_contract": {"type": "string"},
                    "guardrails": {"type": "array", "items": {"type": "object"}},
                    "artifact": {"type": "string", "description": "Content of the produced artifact"}
                },
                "required": ["job_id", "definition_of_done", "artifact"]
            }
        }),
        json!({
            "name": "harvest_insights",
            "description": "Extracts reusable patterns and lessons learned from a completed theme. Enriches cross-project knowledge base.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "theme_id": {"type": "string"},
                    "theme_name": {"type": "string"},
                    "jobs_summary": {"type": "string"},
                    "metrics": {"type": "object", "description": "Optional theme metrics (total_jobs, retries, etc.)"}
                },
                "required": ["theme_id", "theme_name", "jobs_summary"]
            }
        }),
        json!({
            "name": "approve_and_execute",
            "description": "User has approved the FractalPlan. Returns a phased dispatch order (parallelizable themes per phase) with criticity-aware brain activation flags. Logged in WAL.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project_id": {"type": "string"},
                    "approved_by": {"type": "string"},
                    "plan": {"type": "object", "description": "Optional plan override; default loads from WAL state cache"}
                },
                "required": ["project_id", "approved_by"]
            }
        }),
        json!({
            "name": "sync_reflect",
            "description": "After a worker finishes a job, Hermes calls this to validate. Runs Red-Team audit and returns action: commit | retry | escalate.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project_id": {"type": "string"},
                    "job_id": {"type": "string"},
                    "artifact": {"type": "string", "description": "Worker output (diff, log, summary)"},
                    "definition_of_done": {"type": "string"},
                    "convergence_contract": {"type": "string"}
                },
                "required": ["project_id", "job_id", "artifact", "definition_of_done"]
            }
        }),
        json!({
            "name": "check_jobs_status",
            "description": "Returns current state of a project: commit count, last commit timestamp, theme statuses (pending/running/completed/failed).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project_id": {"type": "string"}
                },
                "required": ["project_id"]
            }
        }),
        json!({
            "name": "rollback",
            "description": "Restores project state to a previous commit. Default: previous commit. Specify target_commit_id to rollback further.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project_id": {"type": "string"},
                    "target_commit_id": {"type": "string", "description": "Optional explicit commit target"},
                    "reason": {"type": "string"}
                },
                "required": ["project_id", "reason"]
            }
        }),
        json!({
            "name": "abort",
            "description": "Emergency stop: marks project as aborted, all subsequent dispatch attempts are blocked. Records reason and timestamp in WAL.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project_id": {"type": "string"},
                    "reason": {"type": "string"}
                },
                "required": ["project_id", "reason"]
            }
        }),
    ];
    Ok(json!({"tools": tools}))
}

/// Dispatch un appel d'outil vers CortexServer.
async fn handle_tools_call<C: LlmClient + Clone>(
    server: &CortexServer<C>,
    params: Option<JsonValue>,
) -> DispatchResult {
    let params = match params {
        Some(p) => p,
        None => {
            return Err(JsonRpcError {
                code: INVALID_PARAMS,
                message: "Missing params for tools/call".to_string(),
                data: None,
            })
        }
    };

    let tool_name = params
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or(JsonRpcError {
            code: INVALID_PARAMS,
            message: "Missing 'name' in tools/call params".to_string(),
            data: None,
        })?;

    let arguments = params
        .get("arguments")
        .cloned()
        .unwrap_or(json!({}));

    match tool_name {
        "get_routing_rules" => handle_get_routing_rules(server).await,
        "intercept_plan" => handle_intercept_plan(server, arguments).await,
        "pre_mortem" => handle_pre_mortem(server, arguments).await,
        "red_team_audit" => handle_red_team_audit(server, arguments).await,
        "harvest_insights" => handle_harvest_insights(server, arguments).await,
        "approve_and_execute" => handle_approve_and_execute(server, arguments).await,
        "sync_reflect" => handle_sync_reflect(server, arguments).await,
        "check_jobs_status" => handle_check_jobs_status(server, arguments).await,
        "rollback" => handle_rollback(server, arguments).await,
        "abort" => handle_abort(server, arguments).await,
        _ => Err(JsonRpcError {
            code: METHOD_NOT_FOUND,
            message: format!("Unknown tool: {}", tool_name),
            data: None,
        }),
    }
}

/// Handler get_routing_rules : retourne les règles pour Hermes.
async fn handle_get_routing_rules<C: LlmClient + Clone>(
    server: &CortexServer<C>,
) -> DispatchResult {
    let resp = server.get_routing_rules().await.map_err(|e| JsonRpcError {
        code: crate::protocol::INTERNAL_ERROR,
        message: format!("get_routing_rules failed: {}", e),
        data: None,
    })?;

    let content = serde_json::to_value(&resp).map_err(|e| JsonRpcError {
        code: crate::protocol::INTERNAL_ERROR,
        message: format!("serialization failed: {}", e),
        data: None,
    })?;

    Ok(json!({"content": [{
        "type": "text",
        "text": serde_json::to_string_pretty(&content).unwrap()
    }]}))
}

/// Handler intercept_plan : génère un plan fractal.
async fn handle_intercept_plan<C: LlmClient + Clone>(
    server: &CortexServer<C>,
    arguments: JsonValue,
) -> DispatchResult {
    let intent = arguments
        .get("intent")
        .and_then(|v| v.as_str())
        .ok_or(JsonRpcError {
            code: INVALID_PARAMS,
            message: "Missing required parameter 'intent' for intercept_plan".to_string(),
            data: None,
        })?
        .to_string();

    let context = arguments
        .get("context")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let project_id = arguments
        .get("project_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let request = InterceptPlanRequest {
        intent,
        context,
        project_id,
    };

    let response = server.intercept_plan(request).await.map_err(|e| JsonRpcError {
        code: crate::protocol::INTERNAL_ERROR,
        message: format!("intercept_plan failed: {}", e),
        data: None,
    })?;

    let content = serde_json::to_value(&response).map_err(|e| JsonRpcError {
        code: crate::protocol::INTERNAL_ERROR,
        message: format!("serialization failed: {}", e),
        data: None,
    })?;

    Ok(json!({"content": [{
        "type": "text",
        "text": serde_json::to_string_pretty(&content).unwrap()
    }]}))
}

/// Handler pre_mortem : génère des guardrails pour un job à criticité élevée.
async fn handle_pre_mortem<C: LlmClient + Clone>(
    server: &CortexServer<C>,
    arguments: JsonValue,
) -> DispatchResult {
    let request =
        serde_json::from_value::<crate::server::PreMortemRequest>(arguments)
            .map_err(|e| JsonRpcError {
                code: INVALID_PARAMS,
                message: format!("Invalid params for pre_mortem: {}", e),
                data: None,
            })?;

    let response = server.pre_mortem(request).await.map_err(|e| JsonRpcError {
        code: crate::protocol::INTERNAL_ERROR,
        message: format!("pre_mortem failed: {}", e),
        data: None,
    })?;

    let content = serde_json::to_value(&response).map_err(|e| JsonRpcError {
        code: crate::protocol::INTERNAL_ERROR,
        message: format!("serialization failed: {}", e),
        data: None,
    })?;

    Ok(json!({"content": [{
        "type": "text",
        "text": serde_json::to_string_pretty(&content).unwrap()
    }]}))
}

/// Handler red_team_audit : audit adversarial d'un artéfact.
async fn handle_red_team_audit<C: LlmClient + Clone>(
    server: &CortexServer<C>,
    arguments: JsonValue,
) -> DispatchResult {
    let request =
        serde_json::from_value::<crate::server::RedTeamAuditRequest>(arguments)
            .map_err(|e| JsonRpcError {
                code: INVALID_PARAMS,
                message: format!("Invalid params for red_team_audit: {}", e),
                data: None,
            })?;

    let response = server
        .red_team_audit(request)
        .await
        .map_err(|e| JsonRpcError {
            code: crate::protocol::INTERNAL_ERROR,
            message: format!("red_team_audit failed: {}", e),
            data: None,
        })?;

    let content = serde_json::to_value(&response).map_err(|e| JsonRpcError {
        code: crate::protocol::INTERNAL_ERROR,
        message: format!("serialization failed: {}", e),
        data: None,
    })?;

    Ok(json!({"content": [{
        "type": "text",
        "text": serde_json::to_string_pretty(&content).unwrap()
    }]}))
}

/// Handler harvest_insights : extrait patterns d'un thème complété.
async fn handle_harvest_insights<C: LlmClient + Clone>(
    server: &CortexServer<C>,
    arguments: JsonValue,
) -> DispatchResult {
    let request =
        serde_json::from_value::<crate::server::HarvestInsightsRequest>(arguments)
            .map_err(|e| JsonRpcError {
                code: INVALID_PARAMS,
                message: format!("Invalid params for harvest_insights: {}", e),
                data: None,
            })?;

    let response = server
        .harvest_insights(request)
        .await
        .map_err(|e| JsonRpcError {
            code: crate::protocol::INTERNAL_ERROR,
            message: format!("harvest_insights failed: {}", e),
            data: None,
        })?;

    let content = serde_json::to_value(&response).map_err(|e| JsonRpcError {
        code: crate::protocol::INTERNAL_ERROR,
        message: format!("serialization failed: {}", e),
        data: None,
    })?;

    Ok(json!({"content": [{
        "type": "text",
        "text": serde_json::to_string_pretty(&content).unwrap()
    }]}))
}

// ----------------------------------------------------------------------------
// Worker lifecycle tools (Session 4)
// ----------------------------------------------------------------------------

/// Helper : package une réponse de tool MCP en `content`/`text`.
fn tool_response<T: serde::Serialize>(response: T) -> DispatchResult {
    let content = serde_json::to_value(&response).map_err(|e| JsonRpcError {
        code: crate::protocol::INTERNAL_ERROR,
        message: format!("serialization failed: {}", e),
        data: None,
    })?;
    Ok(json!({"content": [{
        "type": "text",
        "text": serde_json::to_string_pretty(&content).unwrap()
    }]}))
}

async fn handle_approve_and_execute<C: LlmClient + Clone>(
    server: &CortexServer<C>,
    arguments: JsonValue,
) -> DispatchResult {
    let request = serde_json::from_value::<crate::server::ApproveAndExecuteRequest>(arguments)
        .map_err(|e| JsonRpcError {
            code: INVALID_PARAMS,
            message: format!("Invalid params for approve_and_execute: {}", e),
            data: None,
        })?;
    let response = server.approve_and_execute(request).await.map_err(|e| {
        JsonRpcError {
            code: crate::protocol::INTERNAL_ERROR,
            message: format!("approve_and_execute failed: {}", e),
            data: None,
        }
    })?;
    tool_response(response)
}

async fn handle_sync_reflect<C: LlmClient + Clone>(
    server: &CortexServer<C>,
    arguments: JsonValue,
) -> DispatchResult {
    let request = serde_json::from_value::<crate::server::SyncReflectRequest>(arguments)
        .map_err(|e| JsonRpcError {
            code: INVALID_PARAMS,
            message: format!("Invalid params for sync_reflect: {}", e),
            data: None,
        })?;
    let response = server.sync_reflect(request).await.map_err(|e| JsonRpcError {
        code: crate::protocol::INTERNAL_ERROR,
        message: format!("sync_reflect failed: {}", e),
        data: None,
    })?;
    tool_response(response)
}

async fn handle_check_jobs_status<C: LlmClient + Clone>(
    server: &CortexServer<C>,
    arguments: JsonValue,
) -> DispatchResult {
    let request = serde_json::from_value::<crate::server::CheckJobsStatusRequest>(arguments)
        .map_err(|e| JsonRpcError {
            code: INVALID_PARAMS,
            message: format!("Invalid params for check_jobs_status: {}", e),
            data: None,
        })?;
    let response = server.check_jobs_status(request).await.map_err(|e| {
        JsonRpcError {
            code: crate::protocol::INTERNAL_ERROR,
            message: format!("check_jobs_status failed: {}", e),
            data: None,
        }
    })?;
    tool_response(response)
}

async fn handle_rollback<C: LlmClient + Clone>(
    server: &CortexServer<C>,
    arguments: JsonValue,
) -> DispatchResult {
    let request = serde_json::from_value::<crate::server::RollbackRequest>(arguments)
        .map_err(|e| JsonRpcError {
            code: INVALID_PARAMS,
            message: format!("Invalid params for rollback: {}", e),
            data: None,
        })?;
    let response = server.rollback(request).await.map_err(|e| JsonRpcError {
        code: crate::protocol::INTERNAL_ERROR,
        message: format!("rollback failed: {}", e),
        data: None,
    })?;
    tool_response(response)
}

async fn handle_abort<C: LlmClient + Clone>(
    server: &CortexServer<C>,
    arguments: JsonValue,
) -> DispatchResult {
    let request = serde_json::from_value::<crate::server::AbortRequest>(arguments)
        .map_err(|e| JsonRpcError {
            code: INVALID_PARAMS,
            message: format!("Invalid params for abort: {}", e),
            data: None,
        })?;
    let response = server.abort(request).await.map_err(|e| JsonRpcError {
        code: crate::protocol::INTERNAL_ERROR,
        message: format!("abort failed: {}", e),
        data: None,
    })?;
    tool_response(response)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use cortex_brains::{Architect, MockLlmClient};
    use cortex_core::WalService;

    const MOCK_PLAN: &str = r#"{
        "themes": [{
            "id": "TH-1", "name": "Test", "is_parallel_branch": true,
            "convergence_contract": null, "depends_on": [], "criticity_score": 3,
            "resources_used": ["src/"], "concurrency_group": null,
            "tasks": [{"id": "T-1.1", "name": "T", "definition_of_done": "D", "depends_on": []}]
        }],
        "concurrency_groups": [], "parking_lot": [], "ignored_noise": [], "impact_warnings": []
    }"#;

    async fn build_server() -> CortexServer<MockLlmClient> {
        let wal = WalService::connect("sqlite::memory:").await.unwrap();
        let mock = MockLlmClient::with_response(MOCK_PLAN.to_string());
        CortexServer::new(wal, Architect::new(mock.clone()), mock)
    }

    #[tokio::test]
    async fn test_handle_initialize() {
        let result = handle_initialize().expect("should succeed");
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert_eq!(result["serverInfo"]["name"], "cortex-mcp");
        assert!(result["capabilities"]["tools"]["listChanged"]
            .as_bool()
            .is_some());
    }

    #[tokio::test]
    async fn test_handle_ping() {
        let result = handle_ping().expect("should succeed");
        assert_eq!(result["status"], "ok");
    }

    #[tokio::test]
    async fn test_handle_tools_list() {
        let result = handle_tools_list().expect("should succeed");
        let tools = result["tools"].as_array().expect("should be array");
        assert_eq!(tools.len(), 10);

        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"get_routing_rules"));
        assert!(names.contains(&"intercept_plan"));
        assert!(names.contains(&"pre_mortem"));
        assert!(names.contains(&"red_team_audit"));
        assert!(names.contains(&"harvest_insights"));
    }

    #[tokio::test]
    async fn test_dispatch_get_routing_rules() {
        let server = build_server().await;

        let params = json!({
            "name": "get_routing_rules",
            "arguments": {}
        });

        let result = handle_tools_call(&server, Some(params))
            .await
            .expect("should succeed");

        // MCP expects content array
        let content = result["content"].as_array().expect("should be array");
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");

        // Verify it's valid JSON containing routing rules
        let text = content[0]["text"].as_str().expect("should be string");
        let parsed: JsonValue = serde_json::from_str(text).expect("should parse JSON");
        assert!(parsed["rules"]["enabled"].as_bool().is_some());
    }

    #[tokio::test]
    async fn test_dispatch_intercept_plan() {
        let server = build_server().await;

        let params = json!({
            "name": "intercept_plan",
            "arguments": {
                "intent": "Refactorise le module auth",
                "context": "Projet FastAPI"
            }
        });

        let result = handle_tools_call(&server, Some(params))
            .await
            .expect("should succeed");

        let content = result["content"].as_array().expect("should be array");
        let text = content[0]["text"].as_str().expect("should be string");
        let parsed: JsonValue = serde_json::from_str(text).expect("should parse");

        assert!(parsed["project_id"].as_str().unwrap().starts_with("project-"));
        assert_eq!(parsed["plan"]["themes"].as_array().unwrap().len(), 1);
        assert_eq!(parsed["requires_user_approval"], true);
    }

    #[tokio::test]
    async fn test_dispatch_intercept_plan_missing_intent() {
        let server = build_server().await;

        let params = json!({
            "name": "intercept_plan",
            "arguments": {"context": "no intent"}
        });

        let err = handle_tools_call(&server, Some(params))
            .await
            .expect_err("should fail on missing intent");

        assert_eq!(err.code, INVALID_PARAMS);
        assert!(err.message.contains("intent"));
    }

    #[tokio::test]
    async fn test_dispatch_unknown_tool() {
        let server = build_server().await;

        let params = json!({
            "name": "unknown_tool",
            "arguments": {}
        });

        let err = handle_tools_call(&server, Some(params))
            .await
            .expect_err("should fail on unknown tool");

        assert_eq!(err.code, METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn test_dispatch_tools_call_missing_params() {
        let server = build_server().await;

        let err = handle_tools_call(&server, None)
            .await
            .expect_err("should fail on missing params");

        assert_eq!(err.code, INVALID_PARAMS);
    }

    #[tokio::test]
    async fn test_dispatch_method_not_found() {
        let server = build_server().await;

        let result = dispatch(&server, "unknown/method", None)
            .await
            .expect("should return Some for unknown method")
            .expect_err("should be error");

        assert_eq!(result.code, METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn test_dispatch_notification_ignored() {
        let server = build_server().await;

        let result = dispatch(&server, "notifications/initialized", None).await;
        assert!(result.is_none(), "Notifications should return None");
    }
}
