//! Pre-Mortem brain (Simulateur Paranoïaque).
//!
//! Anticipe les modes d'échec d'un job en utilisant le catalogue F1-F19 d'Hermes
//! et génère des **executable_guardrails** — pré-conditions vérifiables qu'un
//! worker doit respecter.
//!
//! # Guardrails
//!
//! Chaque guardrail contient :
//! - `id` : ex `G-1.1`
//! - `description` : pré-condition binaire (ex: "Le fichier existe avant lecture")
//! - `failure_mode` : référence au catalogue F1-F19 (ex: `F7` = file not found)
//! - `check_command` (optionnel) : commande bash pour vérification automatique
//!
//! # Activation
//!
//! Le Pre-Mortem n'est activé que pour les jobs avec criticité ≥ 4
//! (cf. `CostGating::should_run_pre_mortem`).

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::llm_client::{LlmClient, LlmError, LlmRequest};

// ============================================================================
// Types
// ============================================================================

/// Guardrail exécutable généré par le Pre-Mortem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutableGuardrail {
    pub id: String,
    pub description: String,
    pub failure_mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_command: Option<String>,
}

/// Résumé de la simulation Pre-Mortem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreMortemResult {
    pub job_id: String,
    pub guardrails: Vec<ExecutableGuardrail>,
    pub risk_assessment: String,
    pub estimated_risk_score: u8,
}

/// Erreurs spécifiques au Pre-Mortem.
#[derive(Error, Debug)]
pub enum PreMortemError {
    #[error("LLM call failed: {0}")]
    LlmCallFailed(#[from] LlmError),
    #[error("Response parsing failed: {0}")]
    ResponseParsingFailed(String),
    #[error("Invalid guardrail: {0}")]
    InvalidGuardrail(String),
}

// ============================================================================
// Brain
// ============================================================================

/// Pre-Mortem brain : génère guardrails pour prévenir les échecs.
pub struct PreMortem<C: LlmClient> {
    client: C,
}

impl<C: LlmClient> PreMortem<C> {
    pub fn new(client: C) -> Self {
        Self { client }
    }

    /// Génère les guardrails pour un job donné.
    ///
    /// # Arguments
    /// - `job_id` : ID du job
    /// - `job_description` : ce que le job doit faire
    /// - `definition_of_done` : critères de succès
    /// - `context` : environnement, contraintes
    pub async fn generate_guardrails(
        &self,
        job_id: &str,
        job_description: &str,
        definition_of_done: &str,
        context: &str,
    ) -> Result<PreMortemResult, PreMortemError> {
        let system = build_system_prompt();
        let user = build_user_prompt(job_id, job_description, definition_of_done, context);

        let request = LlmRequest {
            system,
            user,
            max_tokens: Some(2000),
            temperature: Some(0.2),
            model: None,
        };

        let response = self.client.complete(request).await?;
        let guardrails = parse_guardrails_response(&response.text)?;
        validate_guardrails(&guardrails)?;

        // Calculer risk_score = max(failure_mode severity)
        let risk_score = compute_risk_score(&guardrails);

        let risk_assessment = build_risk_assessment(&guardrails);

        Ok(PreMortemResult {
            job_id: job_id.to_string(),
            guardrails,
            risk_assessment,
            estimated_risk_score: risk_score,
        })
    }
}

// ============================================================================
// Prompts
// ============================================================================

fn build_system_prompt() -> String {
    r#"Tu es le Simulateur Paranoïaque (Pre-Mortem). Ton rôle : anticiper
TOUS les modes d'échec possibles pour un job technique avant qu'il ne soit exécuté.

## CATALOGUE F1-F19 (modes d'échec Hermes)

- F1 : Process orphan (worker crash)
- F2 : File lock contention
- F3 : Git conflict / push rejection
- F4 : Dependency install failure
- F5 : Port already in use
- F6 : Timeout exceeded
- F7 : File not found / permission denied
- F8 : Database migration failure
- F9 : API rate limit (429)
- F10 : Schema validation error
- F11 : Test suite regression
- F12 : Merge conflict
- F13 : Environment variable missing
- F14 : Disk space / memory exhaustion
- F15 : Network timeout / DNS failure
- F16 : Type error / compilation failure
- F17 : Infinite loop / deadlock
- F18 : Stale cache state
- F19 : Partial state (mid-mutation crash)

## RÈGLES

1. Génère 3 à 7 guardrails. Chaque guardrail doit être :
   - **Binaire** (vrai/faux, vérifiable)
   - **Spécifique** au job (pas générique "vérifier que ça marche")
   - **Actionnable** : si violated, Hermes sait quoi faire

2. Lie chaque guardrail à UN mode d'échec F1-F19 le plus probable.

3. Pour chaque guardrail, propose une `check_command` bash (si pertinent) :
   - Commande retournant exit code 0 si OK, non-zero si violation
   - Doit être sans effet de bord (lecture seule)

4. Calcule un risque global (1-5) basé sur :
   - Nombre de guardrails (beaucoup = complexe)
   - Criticité des modes d'échec (F3, F8, F11, F19 sont hauts)

## FORMAT JSON

```json
{
  "guardrails": [
    {
      "id": "G-1.1",
      "description": "Le fichier config.yaml existe avant lecture",
      "failure_mode": "F7",
      "check_command": "test -f config.yaml"
    },
    {
      "id": "G-1.2",
      "description": "Pas de processus python déjà en écoute sur port 8000",
      "failure_mode": "F5",
      "check_command": "! (lsof -i :8000 | grep -q LISTEN)"
    }
  ]
}
```"#
        .to_string()
}

fn build_user_prompt(
    job_id: &str,
    job_description: &str,
    definition_of_done: &str,
    context: &str,
) -> String {
    format!(
        r#"JOB ID : {}

JOB DESCRIPTION :
{}

DEFINITION OF DONE :
{}

CONTEXTE :
{}

---

Génère les guardrails au format JSON ci-dessus."#,
        job_id, job_description, definition_of_done, context
    )
}

// ============================================================================
// Parsing
// ============================================================================

fn parse_guardrails_response(text: &str) -> Result<Vec<ExecutableGuardrail>, PreMortemError> {
    #[derive(Deserialize)]
    struct Wrapper {
        guardrails: Vec<ExecutableGuardrail>,
    }

    // Try direct parse
    if let Ok(wrapper) = serde_json::from_str::<Wrapper>(text) {
        return Ok(wrapper.guardrails);
    }

    // Try JSON extraction
    let start = text.find('{');
    let end = text.rfind('}');
    match (start, end) {
        (Some(s), Some(e)) if e > s => {
            let json = &text[s..=e];
            serde_json::from_str::<Wrapper>(json)
                .map(|w| w.guardrails)
                .map_err(|err| {
                    PreMortemError::ResponseParsingFailed(format!(
                        "JSON parse: {} (body: {})",
                        err, json
                    ))
                })
        }
        _ => Err(PreMortemError::ResponseParsingFailed(
            "No JSON object found in response".to_string(),
        )),
    }
}

fn validate_guardrails(guardrails: &[ExecutableGuardrail]) -> Result<(), PreMortemError> {
    if guardrails.is_empty() {
        return Err(PreMortemError::InvalidGuardrail(
            "At least one guardrail required".to_string(),
        ));
    }
    if guardrails.len() > 10 {
        return Err(PreMortemError::InvalidGuardrail(format!(
            "Too many guardrails ({}); max 10",
            guardrails.len()
        )));
    }

    for g in guardrails {
        if g.id.is_empty() {
            return Err(PreMortemError::InvalidGuardrail(
                "Guardrail id cannot be empty".to_string(),
            ));
        }
        if g.description.is_empty() {
            return Err(PreMortemError::InvalidGuardrail(format!(
                "Guardrail {} has empty description",
                g.id
            )));
        }
        if g.failure_mode.is_empty() {
            return Err(PreMortemError::InvalidGuardrail(format!(
                "Guardrail {} has no failure_mode",
                g.id
            )));
        }
        // Validate failure_mode starts with F
        if !g.failure_mode.starts_with('F') || g.failure_mode.len() < 2 {
            return Err(PreMortemError::InvalidGuardrail(format!(
                "Guardrail {} has invalid failure_mode '{}'; expected F1-F19",
                g.id, g.failure_mode
            )));
        }
    }

    Ok(())
}

fn compute_risk_score(guardrails: &[ExecutableGuardrail]) -> u8 {
    // Risk score 1-5 based on count + high-severity failures
    let base_score = 1;
    let count_factor = (guardrails.len() as u8).saturating_sub(3).min(2);
    let high_severity: u8 = guardrails
        .iter()
        .filter(|g| matches!(g.failure_mode.as_str(), "F3" | "F8" | "F11" | "F19" | "F14"))
        .count() as u8;
    (base_score + count_factor + high_severity).min(5)
}

fn build_risk_assessment(guardrails: &[ExecutableGuardrail]) -> String {
    format!(
        "{} guardrail(s) : {}",
        guardrails.len(),
        guardrails
            .iter()
            .map(|g| format!("{}[{}]", g.id, g.failure_mode))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_client::MockLlmClient;

    const VALID_RESPONSE: &str = r#"{
        "guardrails": [
            {"id": "G-1.1", "description": "File exists", "failure_mode": "F7", "check_command": "test -f file.txt"},
            {"id": "G-1.2", "description": "DB reachable", "failure_mode": "F8", "check_command": "pg_isready"},
            {"id": "G-1.3", "description": "No port conflict", "failure_mode": "F5"}
        ]
    }"#;

    #[tokio::test]
    async fn test_generate_guardrails_success() {
        let mock = MockLlmClient::with_response(VALID_RESPONSE.to_string());
        let brain = PreMortem::new(mock);

        let result = brain
            .generate_guardrails(
                "J-1.1",
                "Migrate DB users table",
                "All migrations applied",
                "PostgreSQL 15",
            )
            .await
            .expect("should work");

        assert_eq!(result.job_id, "J-1.1");
        assert_eq!(result.guardrails.len(), 3);
        assert_eq!(result.guardrails[0].id, "G-1.1");
        assert_eq!(result.guardrails[0].failure_mode, "F7");
        assert_eq!(
            result.guardrails[0].check_command,
            Some("test -f file.txt".into())
        );
        // F8 is high-severity → risk_score >= 2
        assert!(result.estimated_risk_score >= 2);
    }

    #[tokio::test]
    async fn test_generate_guardrails_preamble() {
        let body = format!("Voici mon analyse...\n\n{}", VALID_RESPONSE);
        let mock = MockLlmClient::with_response(body);
        let brain = PreMortem::new(mock);

        let result = brain
            .generate_guardrails("J-1", "do thing", "ok", "")
            .await
            .expect("should parse JSON from preamble");
        assert_eq!(result.guardrails.len(), 3);
    }

    #[tokio::test]
    async fn test_empty_guardrails_rejected() {
        let mock = MockLlmClient::with_response(r#"{"guardrails": []}"#.to_string());
        let brain = PreMortem::new(mock);

        let err = brain
            .generate_guardrails("J-1", "do", "ok", "")
            .await
            .expect_err("empty guardrails should fail");

        match err {
            PreMortemError::InvalidGuardrail(_) => {}
            other => panic!("wrong: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_invalid_failure_mode_rejected() {
        let body = r#"{"guardrails":[{"id":"G-1","description":"ok","failure_mode":"X999"}]}"#;
        let mock = MockLlmClient::with_response(body.to_string());
        let brain = PreMortem::new(mock);

        let err = brain
            .generate_guardrails("J-1", "do", "ok", "")
            .await
            .expect_err("invalid F code should fail");

        match err {
            PreMortemError::InvalidGuardrail(msg) => {
                assert!(msg.contains("failure_mode"));
            }
            other => panic!("wrong: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_empty_description_rejected() {
        let body = r#"{"guardrails":[{"id":"G-1","description":"","failure_mode":"F7"}]}"#;
        let mock = MockLlmClient::with_response(body.to_string());
        let brain = PreMortem::new(mock);

        let err = brain
            .generate_guardrails("J-1", "do", "ok", "")
            .await
            .expect_err("empty description should fail");

        match err {
            PreMortemError::InvalidGuardrail(msg) => {
                assert!(msg.contains("description"));
            }
            other => panic!("wrong: {:?}", other),
        }
    }

    #[test]
    fn test_compute_risk_score_small() {
        let guardrails = vec![
            ExecutableGuardrail {
                id: "G-1".into(),
                description: "d".into(),
                failure_mode: "F7".into(),
                check_command: None,
            },
            ExecutableGuardrail {
                id: "G-2".into(),
                description: "d".into(),
                failure_mode: "F5".into(),
                check_command: None,
            },
        ];
        assert_eq!(compute_risk_score(&guardrails), 1);
    }

    #[test]
    fn test_compute_risk_score_high() {
        let guardrails = vec![
            ExecutableGuardrail {
                id: "G-1".into(),
                description: "d".into(),
                failure_mode: "F8".into(), // high severity
                check_command: None,
            },
            ExecutableGuardrail {
                id: "G-2".into(),
                description: "d".into(),
                failure_mode: "F19".into(), // high severity
                check_command: None,
            },
            ExecutableGuardrail {
                id: "G-3".into(),
                description: "d".into(),
                failure_mode: "F3".into(), // high severity
                check_command: None,
            },
            ExecutableGuardrail {
                id: "G-4".into(),
                description: "d".into(),
                failure_mode: "F7".into(),
                check_command: None,
            },
            ExecutableGuardrail {
                id: "G-5".into(),
                description: "d".into(),
                failure_mode: "F5".into(),
                check_command: None,
            },
        ];
        // 3 high-severity + count_factor 2 + base 1 = 6 capped to 5
        assert_eq!(compute_risk_score(&guardrails), 5);
    }

    #[test]
    fn test_build_risk_assessment_format() {
        let guardrails = vec![
            ExecutableGuardrail {
                id: "G-1.1".into(),
                description: "d".into(),
                failure_mode: "F7".into(),
                check_command: None,
            },
            ExecutableGuardrail {
                id: "G-1.2".into(),
                description: "d".into(),
                failure_mode: "F5".into(),
                check_command: None,
            },
        ];
        let assessment = build_risk_assessment(&guardrails);
        assert!(assessment.contains("2 guardrail(s)"));
        assert!(assessment.contains("G-1.1[F7]"));
        assert!(assessment.contains("G-1.2[F5]"));
    }

    #[test]
    fn test_system_prompt_contains_catalog() {
        let prompt = build_system_prompt();
        assert!(prompt.contains("F1"));
        assert!(prompt.contains("F19"));
        assert!(prompt.contains("Paranoïaque") || prompt.contains("Pre-Mortem"));
    }

    #[tokio::test]
    async fn test_llm_failure_propagated() {
        let mock = MockLlmClient::with_response("".to_string());
        let brain = PreMortem::new(mock);
        let err = brain
            .generate_guardrails("J", "d", "ok", "")
            .await
            .expect_err("empty response should fail");
        match err {
            PreMortemError::ResponseParsingFailed(_) => {}
            other => panic!("wrong: {:?}", other),
        }
    }
}
