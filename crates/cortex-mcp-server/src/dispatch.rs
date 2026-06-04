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
pub async fn dispatch<C: LlmClient>(
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
                    "intent": {
                        "type": "string",
                        "description": "Raw user intent (free text)"
                    },
                    "context": {
                        "type": "string",
                        "description": "Additional context (constraints, history, etc.)"
                    },
                    "project_id": {
                        "type": "string",
                        "description": "Optional. Existing project ID, or omit to create new project."
                    }
                },
                "required": ["intent"]
            }
        })
    ];
    Ok(json!({"tools": tools}))
}

/// Dispatch un appel d'outil vers CortexServer.
async fn handle_tools_call<C: LlmClient>(
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
        _ => Err(JsonRpcError {
            code: METHOD_NOT_FOUND,
            message: format!("Unknown tool: {}", tool_name),
            data: None,
        }),
    }
}

/// Handler get_routing_rules : retourne les règles pour Hermes.
async fn handle_get_routing_rules<C: LlmClient>(
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
async fn handle_intercept_plan<C: LlmClient>(
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
        CortexServer::new(wal, Architect::new(mock))
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
        assert_eq!(tools.len(), 2);

        let names: Vec<&str> = tools
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"get_routing_rules"));
        assert!(names.contains(&"intercept_plan"));
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
