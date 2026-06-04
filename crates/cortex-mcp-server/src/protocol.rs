//! Protocol JSON-RPC 2.0 pour MCP over stdio.
//!
//! # MCP over stdio
//!
//! MCP utilise JSON-RPC 2.0 avec messages délimités par newlines sur stdout.
//! - Request : `{"jsonrpc":"2.0","method":"...","params":{...},"id":123}`
//! - Response : `{"jsonrpc":"2.0","result":{...},"id":123}`
//! - Error : `{"jsonrpc":"2.0","error":{"code":-32600,"message":"..."},"id":123}`
//!
//! # Logs
//!
//! Les logs Cortex sont envoyés sur stderr (tracing) pour ne pas polluer stdout.
//!
//! # Messages supportés
//!
//! - `initialize` : Handshake (retourne server capabilities)
//! - `tools/list` : Liste les outils disponibles
//! - `tools/call` : Appelle un outil (get_routing_rules, intercept_plan)
//! - `ping` : Keep-alive
//!
//! # Non supporté (Phase 4 MVP)
//!
//! - Notifications (pas d'id) : on les ignore silencieusement
//! - SSE, HTTP : on ne fait que stdio

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

// ============================================================================
// Types JSON-RPC 2.0
// ============================================================================

/// Requête JSON-RPC 2.0.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    #[serde(default)]
    pub method: String,
    #[serde(default)]
    pub params: Option<JsonValue>,
    #[serde(default)]
    pub id: Option<JsonValue>,
}

/// Réponse JSON-RPC 2.0 (succès).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub result: JsonValue,
    pub id: JsonValue,
}

/// Réponse JSON-RPC 2.0 (erreur).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcErrorResponse {
    pub jsonrpc: String,
    pub error: JsonRpcError,
    pub id: JsonValue,
}

/// Corps d'erreur JSON-RPC.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<JsonValue>,
}

// ============================================================================
// Codes d'erreur JSON-RPC standard
// ============================================================================

/// Parse error (JSON invalide).
pub const PARSE_ERROR: i32 = -32700;

/// Invalid request (structure incorrecte).
pub const INVALID_REQUEST: i32 = -32600;

/// Method not found.
pub const METHOD_NOT_FOUND: i32 = -32601;

/// Invalid params.
pub const INVALID_PARAMS: i32 = -32602;

/// Internal server error.
pub const INTERNAL_ERROR: i32 = -32603;

/// Server not initialized (extension MCP).
// Pas unused : réservé pour future impl (extension MCP). Masqué via #[allow].
#[allow(dead_code)]
pub const SERVER_NOT_INITIALIZED: i32 = -32002;

// ============================================================================
// Helpers de construction
// ============================================================================

impl JsonRpcRequest {
    /// Parse une ligne stdin en requête. Ignore les lignes vides/whitespace.
    pub fn parse_line(line: &str) -> Result<Option<Self>, JsonRpcError> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }

        let req: JsonRpcRequest = serde_json::from_str(trimmed).map_err(|e| JsonRpcError {
            code: PARSE_ERROR,
            message: format!("JSON parse error: {}", e),
            data: Some(JsonValue::String(trimmed.to_string())),
        })?;

        if req.jsonrpc != "2.0" {
            return Err(JsonRpcError {
                code: INVALID_REQUEST,
                message: format!("Expected jsonrpc=2.0, got {}", req.jsonrpc),
                data: None,
            });
        }

        Ok(Some(req))
    }

    /// Retourne true si c'est une notification (pas d'id).
    pub fn is_notification(&self) -> bool {
        self.id.is_none()
    }
}

impl JsonRpcResponse {
    /// Construit une réponse succès.
    pub fn success(id: JsonValue, result: JsonValue) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            result,
            id,
        }
    }

    /// Sérialise en une ligne prête à envoyer sur stdout.
    pub fn to_line(&self) -> String {
        let json = serde_json::to_string(self).expect("serialization should succeed");
        format!("{}\n", json)
    }
}

impl JsonRpcErrorResponse {
    /// Construit une réponse erreur.
    pub fn error(id: Option<JsonValue>, code: i32, message: String) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            error: JsonRpcError {
                code,
                message,
                data: None,
            },
            id: id.unwrap_or(JsonValue::Null),
        }
    }

    /// Construit une réponse erreur avec data.
    #[allow(dead_code)] // API publique réservée, utilisée en interne
    pub fn error_with_data(
        id: Option<JsonValue>,
        code: i32,
        message: String,
        data: JsonValue,
    ) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            error: JsonRpcError {
                code,
                message,
                data: Some(data),
            },
            id: id.unwrap_or(JsonValue::Null),
        }
    }

    /// Sérialise en une ligne prête à envoyer sur stdout.
    pub fn to_line(&self) -> String {
        let json = serde_json::to_string(self).expect("serialization should succeed");
        format!("{}\n", json)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_request_valid() {
        let line = r#"{"jsonrpc":"2.0","method":"tools/call","params":{"name":"intercept_plan","arguments":{"intent":"refactor","context":""}},"id":1}"#;
        let req = JsonRpcRequest::parse_line(line)
            .expect("parse should succeed")
            .expect("should be Some");
        assert_eq!(req.method, "tools/call");
        assert_eq!(req.id, Some(JsonValue::Number(1.into())));
    }

    #[test]
    fn test_parse_request_empty_line() {
        let req = JsonRpcRequest::parse_line("").expect("parse empty");
        assert!(req.is_none());

        let req = JsonRpcRequest::parse_line("   ").expect("parse whitespace");
        assert!(req.is_none());
    }

    #[test]
    fn test_parse_request_invalid_json() {
        let err = JsonRpcRequest::parse_line("{not json").expect_err("should fail on invalid json");
        assert_eq!(err.code, PARSE_ERROR);
    }

    #[test]
    fn test_parse_request_wrong_version() {
        let line = r#"{"jsonrpc":"1.0","method":"ping","id":1}"#;
        let err = JsonRpcRequest::parse_line(line).expect_err("should fail on wrong version");
        assert_eq!(err.code, INVALID_REQUEST);
    }

    #[test]
    fn test_parse_notification_no_id() {
        let line = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        let req = JsonRpcRequest::parse_line(line)
            .unwrap()
            .expect("should parse");
        assert!(req.is_notification());
    }

    #[test]
    fn test_response_success_to_line() {
        let resp =
            JsonRpcResponse::success(JsonValue::Number(1.into()), serde_json::json!({"ok": true}));
        let line = resp.to_line();
        assert!(line.starts_with(r#"{"jsonrpc":"2.0","result":{"ok":true},"id":1}"#));
        assert!(line.ends_with('\n'));
    }

    #[test]
    fn test_response_error_to_line() {
        let resp = JsonRpcErrorResponse::error(
            Some(JsonValue::Number(1.into())),
            METHOD_NOT_FOUND,
            "Unknown method".to_string(),
        );
        let line = resp.to_line();
        assert!(line.contains(r#""code":-32601"#));
        assert!(line.contains("Unknown method"));
    }

    #[test]
    fn test_response_error_with_null_id() {
        let resp = JsonRpcErrorResponse::error(None, PARSE_ERROR, "Bad JSON".to_string());
        let line = resp.to_line();
        assert!(line.contains(r#""id":null"#));
    }
}
