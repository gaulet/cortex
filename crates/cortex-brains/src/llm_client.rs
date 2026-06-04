//! LLM client abstraction : trait + types.
//!
//! Le trait `LlmClient` permet d'injecter soit un vrai client HTTP (reqwest),
//! soit un mock pour les tests. Tout le code des brains utilise ce trait,
//! pas reqwest directement.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Erreurs spécifiques aux appels LLM.
#[derive(Error, Debug)]
pub enum LlmError {
    #[error("LLM request failed: {0}")]
    RequestFailed(String),
    #[error("LLM response parsing failed: {0}")]
    ResponseParsingFailed(String),
    #[error("LLM timeout after {0}s")]
    Timeout(u64),
    #[error("LLM rate limited")]
    RateLimited,
    #[error("LLM authentication failed")]
    AuthFailed,
}

/// Requête LLM : prompt + configuration optionnelle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmRequest {
    /// Prompt system (contexte, rôle).
    pub system: String,
    /// Prompt utilisateur (instructions, data).
    pub user: String,
    /// Max tokens à générer (None = défaut modèle).
    pub max_tokens: Option<u32>,
    /// Température (0.0 = déterministe, 1.0 = créatif).
    pub temperature: Option<f32>,
    /// Modèle à utiliser (None = défaut config).
    pub model: Option<String>,
}

impl LlmRequest {
    /// Crée une requête simple avec juste un prompt user.
    pub fn simple(user: String) -> Self {
        Self {
            system: String::new(),
            user,
            max_tokens: None,
            temperature: None,
            model: None,
        }
    }

    /// Crée une requête avec system prompt + user prompt.
    pub fn with_system(system: String, user: String) -> Self {
        Self {
            system,
            user,
            max_tokens: None,
            temperature: None,
            model: None,
        }
    }
}

/// Réponse crue du LLM (texte).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmResponse {
    /// Texte complet généré.
    pub text: String,
    /// Nombre de tokens consommés (input + output), si retourné par l'API.
    pub tokens_used: Option<u32>,
    /// Modèle réellement utilisé (si différent du demandé).
    pub model: Option<String>,
}

/// Client LLM abstrait : tous les brains Cortex l'utilisent via ce trait.
///
/// Exemple d'usage :
/// ```rust,ignore
/// async fn generate_plan(
///     client: &dyn LlmClient,
///     prompt: LlmRequest,
/// ) -> Result<FractalPlan, LlmError> {
///     let response = client.complete(prompt).await?;
///     parse_plan(&response.text)
/// }
/// ```
#[async_trait]
pub trait LlmClient: Send + Sync {
    /// Envoie une requête au LLM et retourne la réponse crue (texte).
    ///
    /// Les brains Cortex parsent ensuite ce texte en structures typées (FractalPlan, etc.)
    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, LlmError>;
}

/// Client LLM mock qui retourne une réponse prédéfinie. Pour tests seulement.
#[derive(Debug, Clone)]
pub struct MockLlmClient {
    response: LlmResponse,
}

impl MockLlmClient {
    /// Crée un mock qui retourne toujours le même texte.
    pub fn with_response(text: String) -> Self {
        Self {
            response: LlmResponse {
                text,
                tokens_used: Some(100),
                model: Some("mock-model".into()),
            },
        }
    }

    /// Crée un mock qui retourne toujours le même texte + tokens.
    pub fn with_full_response(text: String, tokens: u32) -> Self {
        Self {
            response: LlmResponse {
                text,
                tokens_used: Some(tokens),
                model: Some("mock-model".into()),
            },
        }
    }
}

#[async_trait]
impl LlmClient for MockLlmClient {
    async fn complete(&self, _request: LlmRequest) -> Result<LlmResponse, LlmError> {
        Ok(self.response.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_llm_request_simple() {
        let req = LlmRequest::simple("Bonjour".into());
        assert_eq!(req.user, "Bonjour");
        assert_eq!(req.system, "");
        assert!(req.max_tokens.is_none());
    }

    #[test]
    fn test_llm_request_with_system() {
        let req = LlmRequest::with_system("Tu es un agent".into(), "Fais X".into());
        assert_eq!(req.system, "Tu es un agent");
        assert_eq!(req.user, "Fais X");
    }

    #[tokio::test]
    async fn test_mock_llm_client_returns_configured_response() {
        let mock = MockLlmClient::with_response("Réponse test".into());
        let req = LlmRequest::simple("Prompt".into());
        let resp = mock.complete(req).await.expect("mock should succeed");
        assert_eq!(resp.text, "Réponse test");
        assert_eq!(resp.tokens_used, Some(100));
    }

    #[tokio::test]
    async fn test_mock_llm_client_with_custom_tokens() {
        let mock = MockLlmClient::with_full_response("Texte".into(), 42);
        let resp = mock.complete(LlmRequest::simple("".into())).await.unwrap();
        assert_eq!(resp.tokens_used, Some(42));
    }
}
