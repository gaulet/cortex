//! Red-Team brain (Évaluateur Adversarial).
//!
//! Audite l'artéfact produit par un worker sur **5 couches** :
//!
//! 1. **HMAC integrity** : vérifier que les guardrails n'ont pas été altérés
//! 2. **Definition of Done (DoD)** : chaque critère est-il satisfait ?
//! 3. **Executable guardrails** : tous les check_commands passent-ils ?
//! 4. **Workflow coherence** : le convergence_contract est-il respecté ?
//! 5. **Edge cases attack** : recherche de conditions aux limites non couvertes
//!
//! # Activation
//!
//! Red-Team est activé pour tous les jobs avec criticité ≥ 3
//! (cf. `CostGating::should_run_red_team`).

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::llm_client::{LlmClient, LlmError, LlmRequest};

// ============================================================================
// Types
// ============================================================================

/// Sévérité d'un problème détecté par le Red-Team.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Critical,
    High,
    Medium,
    Low,
}

/// Un problème détecté lors de l'audit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditIssue {
    pub layer: String,
    pub severity: Severity,
    pub description: String,
    pub suggestion: Option<String>,
}

/// Résultat global de l'audit Red-Team.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedTeamResult {
    pub job_id: String,
    pub passed: bool,
    pub issues: Vec<AuditIssue>,
    pub approved_layers: Vec<String>,
    pub summary: String,
}

/// Erreurs Red-Team.
#[derive(Error, Debug)]
pub enum RedTeamError {
    #[error("LLM call failed: {0}")]
    LlmCallFailed(#[from] LlmError),
    #[error("Response parsing failed: {0}")]
    ResponseParsingFailed(String),
    #[error("Required field missing: {0}")]
    MissingField(String),
}

// ============================================================================
// Brain
// ============================================================================

/// Red-Team brain : validation adversarial d'artéfacts.
pub struct RedTeam<C: LlmClient> {
    client: C,
}

impl<C: LlmClient> RedTeam<C> {
    pub fn new(client: C) -> Self {
        Self { client }
    }

    /// Vérifie l'intégrité HMAC des guardrails (couche 1, non-LLM).
    ///
    /// Si `expected_signature` est fourni ET `hmac_secret` est fourni :
    /// - Recalcule le HMAC sur `{job_id, guardrails, definition_of_done}`
    /// - Compare avec la signature attendue (constant-time via string equality)
    /// - Retourne Ok(()) si match, Err(RedTeamError) sinon
    ///
    /// Si l'un des deux est None : warning + skip vérification.
    pub fn verify_hmac(
        job_id: &str,
        guardrails: &[crate::paranoiac::ExecutableGuardrail],
        definition_of_done: &str,
        hmac_secret: Option<&[u8]>,
        expected_signature: Option<&str>,
    ) -> Result<(), RedTeamError> {
        match (hmac_secret, expected_signature) {
            (Some(secret), Some(expected)) => {
                let payload = serde_json::json!({
                    "job_id": job_id,
                    "guardrails": guardrails,
                    "definition_of_done": definition_of_done,
                });
                cortex_security::verify_guardrails(secret, &payload, expected).map_err(|e| {
                    RedTeamError::MissingField(format!(
                        "HMAC verification failed: {} — guardrails may have been tampered with",
                        e
                    ))
                })
            }
            (None, Some(_)) => {
                tracing::warn!(
                    "Guardrails have HMAC signature but no server secret configured — skipping verify"
                );
                Ok(())
            }
            (Some(_), None) => {
                tracing::warn!(
                    "Server has HMAC secret but guardrails are unsigned — possible downgrade attack"
                );
                Ok(())
            }
            (None, None) => Ok(()), // Nothing to verify
        }
    }

    /// Audite un artéfact produit par un worker.
    ///
    /// # Arguments
    /// - `job_id` : ID du job audité
    /// - `definition_of_done` : critères attendus
    /// - `convergence_contract` : contrat de sortie (si thème parallélisé)
    /// - `guardrails` : liste de guardrails (HMAC signed en production)
    /// - `artifact` : contenu de l'artéfact (code, documentation, output)
    pub async fn audit(
        &self,
        job_id: &str,
        definition_of_done: &str,
        convergence_contract: Option<&str>,
        guardrails: &[crate::paranoiac::ExecutableGuardrail],
        artifact: &str,
    ) -> Result<RedTeamResult, RedTeamError> {
        let system = build_system_prompt();
        let user = build_user_prompt(
            job_id,
            definition_of_done,
            convergence_contract,
            guardrails,
            artifact,
        );

        let request = LlmRequest {
            system,
            user,
            max_tokens: Some(2500),
            temperature: Some(0.1), // low temperature pour audit rigoureux
            model: None,
        };

        let response = self.client.complete(request).await?;
        let result = parse_audit_response(job_id, &response.text)?;
        validate_audit_result(&result)?;

        Ok(result)
    }
}

// ============================================================================
// Prompts
// ============================================================================

fn build_system_prompt() -> String {
    r#"Tu es le Red-Team (Évaluateur Adversarial). Ton rôle : trouver TOUS les
problèmes dans un artéfact produit par un worker, même les plus subtils.

## 5 COUCHES D'AUDIT OBLIGATOIRES

1. **HMAC integrity** : Les guardrails semblent-ils intacts / non altérés ?
2. **Definition of Done (DoD)** : Chaque critère binaire est-il véritablement satisfait ?
3. **Executable guardrails** : Les check_commands (si présents) pourraient-ils échouer ?
4. **Workflow coherence** : L'artéfact respecte-t-il le convergence_contract ?
5. **Edge cases attack** : Conditions aux limites non couvertes ?

## SÉVÉRITÉS

- `critical` : bloquant, job doit être rejeté
- `high` : problème sérieux, doit être corrigé avant merge
- `medium` : amélioration recommandée
- `low` : style / suggestion mineure

## RÈGLES

1. Un SEUL pass/fail global (`passed`).
   - `passed: true` si TOUTES les couches sont OK (ou seulement low/medium)
   - `passed: false` si au moins un critical ou high
2. Liste tous les problèmes (même si `passed: true`).
3. Pour chaque problème, donne une `suggestion` actionnable quand possible.
4. `approved_layers` : liste des couches validées sans problèmes.
5. `summary` : une phrase résumant l'audit (max 200 caractères).

## FORMAT JSON

```json
{
  "passed": false,
  "approved_layers": ["hmac_integrity"],
  "summary": "DoD non respecté : test coverage insuffisant",
  "issues": [
    {
      "layer": "definition_of_done",
      "severity": "critical",
      "description": "Coverage 50% < 80% requis",
      "suggestion": "Ajouter tests pour edge cases"
    },
    {
      "layer": "edge_cases_attack",
      "severity": "medium",
      "description": "Pas de gestion du cas fichier vide",
      "suggestion": "Ajouter test empty_file.txt"
    }
  ]
}
```"#
        .to_string()
}

fn build_user_prompt(
    job_id: &str,
    definition_of_done: &str,
    convergence_contract: Option<&str>,
    guardrails: &[crate::paranoiac::ExecutableGuardrail],
    artifact: &str,
) -> String {
    let guardrails_text = if guardrails.is_empty() {
        "(aucun guardrail)".to_string()
    } else {
        guardrails
            .iter()
            .map(|g| {
                format!(
                    "- {} : {} [{}] {}",
                    g.id,
                    g.description,
                    g.failure_mode,
                    g.check_command
                        .as_deref()
                        .map(|c| format!("(check: {})", c))
                        .unwrap_or_default()
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    format!(
        r#"JOB ID : {}

DEFINITION OF DONE :
{}

CONVERGENCE CONTRACT :
{}

GUARDRAILS :
{}

ARTIFACT :
{}"#,
        job_id,
        definition_of_done,
        convergence_contract.unwrap_or("(pas de convergence contract)"),
        guardrails_text,
        artifact
    )
}

// ============================================================================
// Parsing
// ============================================================================

fn parse_audit_response(job_id: &str, text: &str) -> Result<RedTeamResult, RedTeamError> {
    #[derive(Deserialize)]
    struct Wrapper {
        passed: bool,
        #[serde(default)]
        approved_layers: Vec<String>,
        #[serde(default)]
        summary: String,
        #[serde(default)]
        issues: Vec<AuditIssue>,
    }

    let wrapper = parse_json::<Wrapper>(text)?;

    Ok(RedTeamResult {
        job_id: job_id.to_string(),
        passed: wrapper.passed,
        issues: wrapper.issues,
        approved_layers: wrapper.approved_layers,
        summary: wrapper.summary,
    })
}

fn parse_json<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, RedTeamError> {
    if let Ok(parsed) = serde_json::from_str::<T>(text) {
        return Ok(parsed);
    }

    let start = text.find('{').ok_or(RedTeamError::ResponseParsingFailed(
        "No JSON object found".to_string(),
    ))?;
    let end = text.rfind('}').ok_or(RedTeamError::ResponseParsingFailed(
        "No closing braces".to_string(),
    ))?;
    if end <= start {
        return Err(RedTeamError::ResponseParsingFailed(
            "Invalid JSON boundaries".to_string(),
        ));
    }

    serde_json::from_str::<T>(&text[start..=end])
        .map_err(|e| RedTeamError::ResponseParsingFailed(format!("JSON parse: {}", e)))
}

fn validate_audit_result(result: &RedTeamResult) -> Result<(), RedTeamError> {
    if !result.passed {
        // If passed=false, must have at least one critical or high issue
        let has_blocker = result
            .issues
            .iter()
            .any(|i| matches!(i.severity, Severity::Critical | Severity::High));
        if !has_blocker && !result.issues.is_empty() {
            return Err(RedTeamError::MissingField(
                "passed=false requires at least one critical/high issue".to_string(),
            ));
        }
    }

    for issue in &result.issues {
        if issue.description.is_empty() {
            return Err(RedTeamError::MissingField(
                "Issue description cannot be empty".to_string(),
            ));
        }
    }

    Ok(())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_client::MockLlmClient;
    use crate::paranoiac::ExecutableGuardrail;

    #[test]
    fn test_verify_hmac_no_secret_no_sig_ok() {
        let result =
            RedTeam::<MockLlmClient>::verify_hmac("J-1", &sample_guardrails(), "DoD", None, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_verify_hmac_matching_sig_ok() {
        let guardrails = sample_guardrails();
        let secret = b"super-secret-hmac-key";
        let payload = serde_json::json!({
            "job_id": "J-1",
            "guardrails": &guardrails,
            "definition_of_done": "DoD",
        });
        let sig = cortex_security::sign_guardrails(secret, &payload).unwrap();
        let result = RedTeam::<MockLlmClient>::verify_hmac(
            "J-1",
            &guardrails,
            "DoD",
            Some(secret),
            Some(&sig),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_verify_hmac_tampered_guardrails_fails() {
        let guardrails = sample_guardrails();
        let secret = b"super-secret-hmac-key";
        let payload = serde_json::json!({
            "job_id": "J-1",
            "guardrails": &guardrails,
            "definition_of_done": "DoD",
        });
        let sig = cortex_security::sign_guardrails(secret, &payload).unwrap();
        // Mutate guardrails
        let mut tampered = guardrails.clone();
        tampered[0].description = "MALICIOUS".into();
        let result = RedTeam::<MockLlmClient>::verify_hmac(
            "J-1",
            &tampered,
            "DoD",
            Some(secret),
            Some(&sig),
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_hmac_wrong_secret_fails() {
        let guardrails = sample_guardrails();
        let payload = serde_json::json!({
            "job_id": "J-1",
            "guardrails": &guardrails,
            "definition_of_done": "DoD",
        });
        let sig = cortex_security::sign_guardrails(b"right-secret", &payload).unwrap();
        let result = RedTeam::<MockLlmClient>::verify_hmac(
            "J-1",
            &guardrails,
            "DoD",
            Some(b"wrong-secret"),
            Some(&sig),
        );
        assert!(result.is_err());
    }

    fn sample_guardrails() -> Vec<ExecutableGuardrail> {
        vec![
            ExecutableGuardrail {
                id: "G-1.1".into(),
                description: "File exists".into(),
                failure_mode: "F7".into(),
                check_command: Some("test -f file.txt".into()),
            },
            ExecutableGuardrail {
                id: "G-1.2".into(),
                description: "DB reachable".into(),
                failure_mode: "F8".into(),
                check_command: None,
            },
        ]
    }

    #[tokio::test]
    async fn test_audit_passed_no_issues() {
        let response = r#"{
            "passed": true,
            "approved_layers": ["hmac_integrity", "definition_of_done", "workflow_coherence"],
            "summary": "Tous critères validés",
            "issues": []
        }"#;
        let mock = MockLlmClient::with_response(response.to_string());
        let brain = RedTeam::new(mock);

        let result = brain
            .audit(
                "J-1.1",
                "Coverage > 80%",
                None,
                &sample_guardrails(),
                "code sample",
            )
            .await
            .unwrap();

        assert!(result.passed);
        assert!(result.issues.is_empty());
        assert_eq!(result.approved_layers.len(), 3);
    }

    #[tokio::test]
    async fn test_audit_failed_with_critical_issue() {
        let response = r#"{
            "passed": false,
            "approved_layers": ["hmac_integrity"],
            "summary": "DoD non respecté",
            "issues": [
                {"layer": "definition_of_done", "severity": "critical", "description": "Coverage 30% < 80%"}
            ]
        }"#;
        let mock = MockLlmClient::with_response(response.to_string());
        let brain = RedTeam::new(mock);

        let result = brain
            .audit("J-1.1", "Coverage > 80%", None, &[], "code")
            .await
            .unwrap();

        assert!(!result.passed);
        assert_eq!(result.issues.len(), 1);
        assert_eq!(result.issues[0].severity, Severity::Critical);
    }

    #[tokio::test]
    async fn test_audit_with_preamble() {
        let response = format!(
            "<red_team_analysis>My analysis...</red_team_analysis>\n\n{}",
            r#"{"passed": true, "issues": [], "approved_layers": [], "summary": "ok"}"#
        );
        let mock = MockLlmClient::with_response(response);
        let brain = RedTeam::new(mock);

        let result = brain
            .audit("J", "x", None, &[], "y")
            .await
            .expect("should parse with preamble");
        assert!(result.passed);
    }

    #[tokio::test]
    async fn test_parse_failure_empty_response() {
        let mock = MockLlmClient::with_response("".to_string());
        let brain = RedTeam::new(mock);

        let err = brain
            .audit("J", "x", None, &[], "y")
            .await
            .expect_err("empty should fail");

        match err {
            RedTeamError::ResponseParsingFailed(_) => {}
            other => panic!("wrong: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_validate_passed_false_requires_blocker() {
        let response = r#"{
            "passed": false,
            "approved_layers": [],
            "summary": "ok?",
            "issues": [
                {"layer": "x", "severity": "low", "description": "minor"}
            ]
        }"#;
        let mock = MockLlmClient::with_response(response.to_string());
        let brain = RedTeam::new(mock);

        let err = brain
            .audit("J", "x", None, &[], "y")
            .await
            .expect_err("passed=false without blocker should fail validation");

        match err {
            RedTeamError::MissingField(_) => {}
            other => panic!("wrong: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_audit_with_convergence_contract() {
        let response = r#"{
            "passed": true,
            "approved_layers": ["workflow_coherence"],
            "summary": "Convergence contract respecté",
            "issues": []
        }"#;
        let mock = MockLlmClient::with_response(response.to_string());
        let brain = RedTeam::new(mock);

        let result = brain
            .audit(
                "J-1.1",
                "Output JSON",
                Some("Must output valid JSON with field X"),
                &[],
                r#"{"x": 1}"#,
            )
            .await
            .unwrap();

        assert!(result.passed);
    }

    #[test]
    fn test_system_prompt_contains_layers() {
        let prompt = build_system_prompt();
        assert!(prompt.contains("HMAC integrity"));
        assert!(prompt.contains("Definition of Done"));
        assert!(prompt.contains("Workflow coherence"));
        assert!(prompt.contains("Edge cases attack"));
        assert!(prompt.contains("severity"));
    }

    #[test]
    fn test_user_prompt_contains_guardrails() {
        let prompt =
            build_user_prompt("J-1", "DoD", None, &sample_guardrails(), "artifact content");
        assert!(prompt.contains("G-1.1"));
        assert!(prompt.contains("G-1.2"));
        assert!(prompt.contains("check: test -f file.txt"));
        assert!(prompt.contains("artifact content"));
    }

    #[test]
    fn test_severity_serialization_roundtrip() {
        let issue = AuditIssue {
            layer: "definition_of_done".into(),
            severity: Severity::Critical,
            description: "x".into(),
            suggestion: Some("y".into()),
        };
        let json = serde_json::to_string(&issue).unwrap();
        assert!(json.contains(r#""severity":"critical""#));
        let parsed: AuditIssue = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.severity, Severity::Critical);
    }
}
