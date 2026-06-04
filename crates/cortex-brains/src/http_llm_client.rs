//! HttpLlmClient : real HTTP implementation of LlmClient trait.
//!
//! Compatible with OpenAI-compatible APIs (OpenRouter, OpenAI, Together, local ollama/vllm).
//!
//! # Configuration
//!
//! ```rust,ignore
//! let client = HttpLlmClient::new(
//!     "sk-xxx",                           // API key
//!     "https://openrouter.ai/api/v1",     // Base URL
//!     "anthropic/claude-sonnet-4",        // Default model
//! );
//! ```
//!
//! # Request format (OpenAI-compatible)
//!
//! ```json
//! {
//!   "model": "...",
//!   "messages": [
//!     {"role": "system", "content": "..."},
//!     {"role": "user", "content": "..."}
//!   ],
//!   "temperature": 0.3,
//!   "max_tokens": 4000
//! }
//! ```
//!
//! # Response format (OpenAI-compatible)
//!
//! ```json
//! {
//!   "id": "chatcmpl-xxx",
//!   "model": "...",
//!   "choices": [
//!     {"message": {"role": "assistant", "content": "..."}}
//!   ],
//!   "usage": {"prompt_tokens": 100, "completion_tokens": 200, "total_tokens": 300}
//! }
//! ```
//!
//! # Error mapping
//!
//! - HTTP timeout → `LlmError::Timeout`
//! - HTTP 429 → `LlmError::RateLimited`
//! - HTTP 401/403 → `LlmError::AuthFailed`
//! - HTTP 4xx/5xx → `LlmError::RequestFailed`
//! - JSON parse error → `LlmError::ResponseParsingFailed`

use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::llm_client::{LlmClient, LlmError, LlmRequest, LlmResponse};

/// HTTP LLM client configured pour un provider OpenAI-compatible.
#[derive(Clone)]
pub struct HttpLlmClient {
    client: reqwest::Client,
    api_key: String,
    base_url: String,
    default_model: String,
    /// Effort de raisonnement étendu (OpenRouter, Anthropic thinking, o1-style).
    /// Valeurs acceptées : "low" | "medium" | "high" | "max". None = désactivé.
    reasoning_effort: Option<String>,
}

// ============================================================================
// OpenAI-compatible request/response types
// ============================================================================

#[derive(Debug, Serialize)]
pub(crate) struct OpenAiRequest {
    model: String,
    messages: Vec<OpenAiMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    /// OpenRouter / Anthropic-style extended reasoning.
    /// Format: { "effort": "max" } ou { "max_tokens": 16000 }
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<OpenAiReasoning>,
}

#[derive(Debug, Serialize, Clone)]
struct OpenAiReasoning {
    /// "low" | "medium" | "high" | "max"
    effort: String,
}

#[derive(Debug, Serialize)]
struct OpenAiMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct OpenAiResponse {
    model: Option<String>,
    choices: Vec<OpenAiChoice>,
    usage: Option<OpenAiUsage>,
}

#[derive(Debug, Deserialize)]
struct OpenAiChoice {
    message: OpenAiMessageResponse,
}

#[derive(Debug, Deserialize)]
struct OpenAiMessageResponse {
    content: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OpenAiUsage {
    total_tokens: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct OpenAiErrorBody {
    error: Option<OpenAiErrorDetail>,
}

#[derive(Debug, Deserialize)]
struct OpenAiErrorDetail {
    message: Option<String>,
}

// ============================================================================
// Implementation
// ============================================================================

impl HttpLlmClient {
    /// Create a new HTTP client.
    ///
    /// - `api_key` : bearer token
    /// - `base_url` : ex `https://openrouter.ai/api/v1` ou `http://localhost:11434/v1`
    /// - `default_model` : utilisé si `LlmRequest.model` est None
    pub fn new(
        api_key: impl Into<String>,
        base_url: impl Into<String>,
        default_model: impl Into<String>,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .expect("failed to build reqwest client");

        Self {
            client,
            api_key: api_key.into(),
            base_url: base_url.into(),
            default_model: default_model.into(),
            reasoning_effort: None,
        }
    }

    /// Change le timeout (en secondes).
    pub fn with_timeout(self, timeout_secs: u64) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .build()
            .expect("failed to build reqwest client");
        Self { client, ..self }
    }

    /// Active le raisonnement étendu avec l'effort spécifié.
    ///
    /// Valeurs acceptées : "low" | "medium" | "high" | "max"
    /// None ou "" → désactive.
    ///
    /// Injecte `"reasoning": { "effort": "..." }` dans le body de la requête.
    /// Compatible OpenRouter (tous les modèles reasoning-enabled), Anthropic
    /// (thinking), et o1-style providers.
    pub fn with_reasoning(mut self, effort: Option<&str>) -> Self {
        self.reasoning_effort = effort
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        self
    }

    /// Build OpenAI request body from LlmRequest.
    pub(crate) fn build_request_body(&self, req: &LlmRequest) -> OpenAiRequest {
        let mut messages = Vec::with_capacity(2);
        if !req.system.is_empty() {
            messages.push(OpenAiMessage {
                role: "system".to_string(),
                content: req.system.clone(),
            });
        }
        messages.push(OpenAiMessage {
            role: "user".to_string(),
            content: req.user.clone(),
        });

        OpenAiRequest {
            model: req
                .model
                .clone()
                .unwrap_or_else(|| self.default_model.clone()),
            messages,
            temperature: req.temperature,
            max_tokens: req.max_tokens,
            reasoning: self
                .reasoning_effort
                .as_ref()
                .map(|effort| OpenAiReasoning {
                    effort: effort.clone(),
                }),
        }
    }

    /// Endpoint URL.
    fn endpoint(&self) -> String {
        let base = self.base_url.trim_end_matches('/');
        format!("{}/chat/completions", base)
    }

    /// Parse response body into LlmResponse.
    pub(crate) fn parse_response_body(body: &str) -> Result<LlmResponse, LlmError> {
        let resp: OpenAiResponse = serde_json::from_str(body).map_err(|e| {
            LlmError::ResponseParsingFailed(format!("JSON parse error: {}\nBody: {}", e, body))
        })?;

        if resp.choices.is_empty() {
            return Err(LlmError::ResponseParsingFailed(
                "Response has no choices".to_string(),
            ));
        }

        let text = resp
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.content)
            .unwrap_or_default();

        Ok(LlmResponse {
            text,
            tokens_used: resp.usage.and_then(|u| u.total_tokens),
            model: resp.model,
        })
    }
}

#[async_trait]
impl LlmClient for HttpLlmClient {
    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, LlmError> {
        let body = self.build_request_body(&request);
        let endpoint = self.endpoint();

        tracing::debug!(
            model = &body.model,
            temperature = body.temperature,
            max_tokens = body.max_tokens,
            "LLM request"
        );

        let response = self
            .client
            .post(&endpoint)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                if e.is_timeout() {
                    LlmError::Timeout(60)
                } else if e.is_connect() {
                    LlmError::RequestFailed(format!("Connection failed: {}", e))
                } else {
                    LlmError::RequestFailed(e.to_string())
                }
            })?;

        let status = response.status();
        let body_str = response.text().await.unwrap_or_default();

        if !status.is_success() {
            return Err(match status.as_u16() {
                429 => LlmError::RateLimited,
                401 | 403 => LlmError::AuthFailed,
                code if code >= 500 => {
                    LlmError::RequestFailed(format!("HTTP {}: {}", code, body_str))
                }
                code => {
                    // Try to parse error details
                    #[allow(clippy::unnecessary_lazy_evaluations)]
                    let detail = serde_json::from_str::<OpenAiErrorBody>(&body_str)
                        .ok()
                        .and_then(|b| b.error)
                        .and_then(|e| e.message)
                        .unwrap_or_else(|| body_str);
                    LlmError::RequestFailed(format!("HTTP {}: {}", code, detail))
                }
            });
        }

        Self::parse_response_body(&body_str)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn test_client() -> HttpLlmClient {
        HttpLlmClient::new("sk-test-key", "https://api.example.com/v1", "gpt-4")
    }

    #[test]
    fn test_endpoint_trailing_slash() {
        let c = HttpLlmClient::new("key", "https://api.example.com/v1/", "gpt-4");
        assert_eq!(c.endpoint(), "https://api.example.com/v1/chat/completions");
    }

    #[test]
    fn test_endpoint_no_trailing_slash() {
        let c = HttpLlmClient::new("key", "https://api.example.com/v1", "gpt-4");
        assert_eq!(c.endpoint(), "https://api.example.com/v1/chat/completions");
    }

    #[test]
    fn test_build_request_body_with_system_and_user() {
        let c = test_client();
        let req = LlmRequest::with_system("system prompt".into(), "user query".into());
        let body = c.build_request_body(&req);

        assert_eq!(body.model, "gpt-4");
        assert_eq!(body.messages.len(), 2);
        assert_eq!(body.messages[0].role, "system");
        assert_eq!(body.messages[0].content, "system prompt");
        assert_eq!(body.messages[1].role, "user");
        assert_eq!(body.messages[1].content, "user query");
    }

    #[test]
    fn test_build_request_body_empty_system() {
        let c = test_client();
        let req = LlmRequest::simple("user only".into());
        let body = c.build_request_body(&req);

        // Empty system → only user message
        assert_eq!(body.messages.len(), 1);
        assert_eq!(body.messages[0].role, "user");
    }

    #[test]
    fn test_build_request_body_uses_provided_model() {
        let c = test_client();
        let mut req = LlmRequest::simple("test".into());
        req.model = Some("claude-3-opus".into());
        let body = c.build_request_body(&req);

        assert_eq!(body.model, "claude-3-opus");
    }

    #[test]
    fn test_build_request_body_temperature_and_max_tokens() {
        let c = test_client();
        let mut req = LlmRequest::simple("test".into());
        req.temperature = Some(0.7);
        req.max_tokens = Some(2000);
        let body = c.build_request_body(&req);

        assert_eq!(body.temperature, Some(0.7));
        assert_eq!(body.max_tokens, Some(2000));
    }

    #[test]
    fn test_serialize_request_body_no_optional_fields() {
        let body = OpenAiRequest {
            model: "gpt-4".into(),
            messages: vec![OpenAiMessage {
                role: "user".into(),
                content: "hi".into(),
            }],
            temperature: None,
            max_tokens: None,
            reasoning: None,
        };
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["model"], "gpt-4");
        assert!(json.get("temperature").is_none());
        assert!(json.get("max_tokens").is_none());
        assert!(json.get("reasoning").is_none());
    }

    #[test]
    fn test_serialize_request_body_with_reasoning() {
        let body = OpenAiRequest {
            model: "minimax/minimax-m3".into(),
            messages: vec![OpenAiMessage {
                role: "user".into(),
                content: "explain".into(),
            }],
            temperature: None,
            max_tokens: None,
            reasoning: Some(OpenAiReasoning {
                effort: "max".into(),
            }),
        };
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["reasoning"]["effort"], "max");
    }

    #[test]
    fn test_with_reasoning_normalizes_input() {
        let c = HttpLlmClient::new("k", "https://x", "m").with_reasoning(Some("  MAX "));
        let body = c.build_request_body(&LlmRequest {
            system: String::new(),
            user: "q".into(),
            model: None,
            temperature: None,
            max_tokens: None,
        });
        assert_eq!(body.reasoning.as_ref().unwrap().effort, "MAX");
    }

    #[test]
    fn test_with_reasoning_empty_disables() {
        let c = HttpLlmClient::new("k", "https://x", "m").with_reasoning(Some(""));
        let body = c.build_request_body(&LlmRequest {
            system: String::new(),
            user: "q".into(),
            model: None,
            temperature: None,
            max_tokens: None,
        });
        assert!(body.reasoning.is_none());
    }

    #[test]
    fn test_request_body_serializes_model_and_messages() {
        let body = OpenAiRequest {
            model: "gpt-4".into(),
            messages: vec![OpenAiMessage {
                role: "user".into(),
                content: "hi".into(),
            }],
            temperature: None,
            max_tokens: None,
            reasoning: None,
        };
        let json = serde_json::to_string(&body).unwrap();
        assert!(json.contains("\"model\":\"gpt-4\""));
        assert!(json.contains("\"user\""));
        assert!(json.contains("\"hi\""));
    }

    #[test]
    fn test_parse_response_body_valid() {
        let json = r#"{
            "model": "gpt-4",
            "choices": [
                {"message": {"role": "assistant", "content": "Hello!"}}
            ],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
        }"#;

        let resp = HttpLlmClient::parse_response_body(json).unwrap();
        assert_eq!(resp.text, "Hello!");
        assert_eq!(resp.tokens_used, Some(15));
        assert_eq!(resp.model, Some("gpt-4".into()));
    }

    #[test]
    fn test_parse_response_body_missing_tokens() {
        let json = r#"{
            "model": "gpt-4",
            "choices": [{"message": {"role": "assistant", "content": "Hi"}}]
        }"#;
        let resp = HttpLlmClient::parse_response_body(json).unwrap();
        assert_eq!(resp.text, "Hi");
        assert_eq!(resp.tokens_used, None);
    }

    #[test]
    fn test_parse_response_body_null_content() {
        let json = r#"{
            "choices": [{"message": {"role": "assistant", "content": null}}]
        }"#;
        let resp = HttpLlmClient::parse_response_body(json).unwrap();
        assert_eq!(resp.text, "");
    }

    #[test]
    fn test_parse_response_body_no_choices_error() {
        let json = r#"{"choices": []}"#;
        let err = HttpLlmClient::parse_response_body(json).unwrap_err();
        match err {
            LlmError::ResponseParsingFailed(msg) => {
                assert!(msg.contains("no choices"));
            }
            other => panic!("wrong error: {:?}", other),
        }
    }

    #[test]
    fn test_parse_response_body_invalid_json() {
        let err = HttpLlmClient::parse_response_body("not json").unwrap_err();
        match err {
            LlmError::ResponseParsingFailed(msg) => {
                assert!(msg.contains("JSON parse error"));
            }
            other => panic!("wrong error: {:?}", other),
        }
    }

    #[test]
    fn test_with_timeout() {
        let c = test_client().with_timeout(120);
        // Just verify it builds successfully (no way to introspect timeout)
        assert_eq!(c.endpoint(), "https://api.example.com/v1/chat/completions");
    }

    #[tokio::test]
    async fn test_complete_connection_failure() {
        // URL that can't be reached → should get RequestFailed
        let c = HttpLlmClient::new("key", "http://127.0.0.1:1", "gpt-4");
        let err = c
            .complete(LlmRequest::simple("hi".into()))
            .await
            .unwrap_err();
        match err {
            LlmError::RequestFailed(msg) => {
                assert!(!msg.is_empty());
            }
            // Some platforms return Timeout for refused connections
            LlmError::Timeout(_) => {} // OK
            other => panic!("unexpected: {:?}", other),
        }
    }
}
