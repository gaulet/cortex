//! Insights Harvester brain.
//!
//! Extrait les patterns réutilisables et leçons apprises d'un thème complété.
//! Suggère des liens inter-projets via Hermes `memory.search`.
//!
//! # Activation
//!
//! Déclenché manuellement via l'outil `harvest_insights` MCP après qu'un thème
//! soit marqué `completed` par Hermes.
//!
//! # Output
//!
//! - Patterns réutilisables (format "quand X, faire Y")
//! - Leçons apprises (format "éviter X car Y")
//! - Métriques de qualité (temps, tokens, taux de retry)
//! - Suggestions cross-projets (projets impactés)

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::llm_client::{LlmClient, LlmError, LlmRequest};

// ============================================================================
// Types
// ============================================================================

/// Pattern réutilisable extrait d'un thème.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReusablePattern {
    pub title: String,
    pub when: String,
    pub then: String,
    pub source_theme: String,
}

/// Leçon apprise (anti-pattern à éviter).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LessonLearned {
    pub title: String,
    pub avoid: String,
    pub reason: String,
    pub source_theme: String,
}

/// Suggestion de lien inter-projet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrossProjectSuggestion {
    pub target_project: String,
    pub relation_type: String,
    pub description: String,
}

/// Métriques de qualité du thème.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ThemeMetrics {
    pub total_jobs: u32,
    pub jobs_with_retries: u32,
    pub avg_retries_per_job: f32,
    pub failure_rate: f32,
    pub tokens_consumed: Option<u32>,
}

/// Résultat complet de la récolte d'insights.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InsightsResult {
    pub theme_id: String,
    pub patterns: Vec<ReusablePattern>,
    pub lessons: Vec<LessonLearned>,
    pub cross_project_suggestions: Vec<CrossProjectSuggestion>,
    pub metrics: ThemeMetrics,
    pub summary: String,
}

/// Erreurs insights harvester.
#[derive(Error, Debug)]
pub enum InsightsError {
    #[error("LLM call failed: {0}")]
    LlmCallFailed(#[from] LlmError),
    #[error("Response parsing failed: {0}")]
    ResponseParsingFailed(String),
}

// ============================================================================
// Brain
// ============================================================================

/// Insights Harvester brain.
pub struct InsightsHarvester<C: LlmClient> {
    client: C,
}

impl<C: LlmClient> InsightsHarvester<C> {
    pub fn new(client: C) -> Self {
        Self { client }
    }

    /// Analyse un thème complété pour en extraire insights réutilisables.
    ///
    /// # Arguments
    /// - `theme_id` : ID du thème
    /// - `theme_name` : nom du thème
    /// - `jobs_summary` : résumé des jobs complétés
    /// - `metrics` : métriques du thème
    pub async fn harvest(
        &self,
        theme_id: &str,
        theme_name: &str,
        jobs_summary: &str,
        metrics: ThemeMetrics,
    ) -> Result<InsightsResult, InsightsError> {
        let system = build_system_prompt();
        let user = build_user_prompt(theme_id, theme_name, jobs_summary);

        let request = LlmRequest {
            system,
            user,
            max_tokens: Some(2500),
            temperature: Some(0.3),
            model: None,
        };

        let response = self.client.complete(request).await?;
        let mut result = parse_insights_response(theme_id, &response.text)?;
        result.metrics = metrics;

        Ok(result)
    }
}

// ============================================================================
// Prompts
// ============================================================================

fn build_system_prompt() -> String {
    r#"Tu es le Récolteur d'Insights. Ton rôle : extraire les patterns réutilisables
et leçons apprises d'un thème complété pour enrichir la base de connaissance
transversale des projets Cortex.

## CE QUE TU DOIS EXTRAIRE

### 1. Patterns réutilisables (reusable patterns)
Format : "quand X (situation), faire Y (action)"
- Doivent être généraux/abstraits (pas spécifiques au projet)
- Actionnables immédiatement
- Exemples :
  - "Quand migration SQL > 10 tables, grouper par domaine fonctionnel"
  - "Quand tests > 5 min, paralléliser via pytest-xdist"

### 2. Leçons apprises (lessons)
Format : "éviter X car Y"
- Erreurs classiques détectées
- Anti-patterns à documenter
- Exemples :
  - "Éviter ALTER TABLE sans rollback car migration non-reversible"
  - "Éviter test HTTP réel, utiliser fixtures"

### 3. Suggestions cross-projets
- Quels projets pourraient bénéficier de ces patterns?
- Quelles synergies/anti-conflits détecter?

## RÈGLES

- 2 à 5 patterns par thème
- 1 à 3 lessons par thème
- 0 à 3 suggestions cross-projets (peut être vide)
- summary : une phrase résumant les insights clés

## FORMAT JSON

```json
{
  "patterns": [
    {"title": "Migration par domaines", "when": "migration SQL", "then": "grouper par domaine", "source_theme": "TH-1"},
    {"title": "Tests parallèles", "when": "tests > 5min", "then": "pytest-xdist", "source_theme": "TH-1"}
  ],
  "lessons": [
    {"title": "Rollback migrations", "avoid": "ALTER TABLE sans backup", "reason": "non-reversible", "source_theme": "TH-1"}
  ],
  "cross_project_suggestions": [
    {"target_project": "proj_mobile", "relation_type": "partage_schema", "description": "Utiliser même migration pattern"}
  ],
  "summary": "3 patterns migration + 1 lesson rollback"
}
```"#
        .to_string()
}

fn build_user_prompt(theme_id: &str, theme_name: &str, jobs_summary: &str) -> String {
    format!(
        r#"THEME ID : {}
THEME NAME : {}

JOBS COMPLETES (résumé) :
{}

---

Extrais les insights réutilisables au format JSON décrit ci-dessus."#,
        theme_id, theme_name, jobs_summary
    )
}

// ============================================================================
// Parsing
// ============================================================================

fn parse_insights_response(theme_id: &str, text: &str) -> Result<InsightsResult, InsightsError> {
    #[derive(Deserialize)]
    struct Wrapper {
        #[serde(default)]
        patterns: Vec<ReusablePattern>,
        #[serde(default)]
        lessons: Vec<LessonLearned>,
        #[serde(default)]
        cross_project_suggestions: Vec<CrossProjectSuggestion>,
        #[serde(default)]
        summary: String,
    }

    let wrapper = parse_json::<Wrapper>(text)?;

    // Inject theme_id si absent dans chaque pattern/lesson
    let patterns = wrapper
        .patterns
        .into_iter()
        .map(|mut p| {
            if p.source_theme.is_empty() {
                p.source_theme = theme_id.to_string();
            }
            p
        })
        .collect();

    let lessons = wrapper
        .lessons
        .into_iter()
        .map(|mut l| {
            if l.source_theme.is_empty() {
                l.source_theme = theme_id.to_string();
            }
            l
        })
        .collect();

    Ok(InsightsResult {
        theme_id: theme_id.to_string(),
        patterns,
        lessons,
        cross_project_suggestions: wrapper.cross_project_suggestions,
        metrics: ThemeMetrics::default(),
        summary: wrapper.summary,
    })
}

fn parse_json<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, InsightsError> {
    if let Ok(parsed) = serde_json::from_str::<T>(text) {
        return Ok(parsed);
    }

    let start = text.find('{').ok_or(InsightsError::ResponseParsingFailed(
        "No JSON object found".to_string(),
    ))?;
    let end = text.rfind('}').ok_or(InsightsError::ResponseParsingFailed(
        "No closing braces".to_string(),
    ))?;
    if end <= start {
        return Err(InsightsError::ResponseParsingFailed(
            "Invalid JSON boundaries".to_string(),
        ));
    }

    serde_json::from_str::<T>(&text[start..=end])
        .map_err(|e| InsightsError::ResponseParsingFailed(format!("JSON parse: {}", e)))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_client::MockLlmClient;

    const VALID_RESPONSE: &str = r#"{
        "patterns": [
            {"title": "Migration par domaines", "when": "migration SQL > 10", "then": "grouper par domaine", "source_theme": ""},
            {"title": "Tests parallèles", "when": "tests > 5 min", "then": "pytest-xdist", "source_theme": ""}
        ],
        "lessons": [
            {"title": "Rollback migrations", "avoid": "ALTER TABLE", "reason": "non-reversible", "source_theme": ""}
        ],
        "cross_project_suggestions": [
            {"target_project": "proj_mobile", "relation_type": "share", "description": "migration pattern"}
        ],
        "summary": "2 patterns + 1 lesson"
    }"#;

    #[tokio::test]
    async fn test_harvest_success() {
        let mock = MockLlmClient::with_response(VALID_RESPONSE.to_string());
        let brain = InsightsHarvester::new(mock);

        let result = brain
            .harvest(
                "TH-1",
                "DB migration",
                "3 jobs: migration users, migration payments, migration logs",
                ThemeMetrics {
                    total_jobs: 3,
                    jobs_with_retries: 1,
                    avg_retries_per_job: 0.33,
                    failure_rate: 0.0,
                    tokens_consumed: Some(15000),
                },
            )
            .await
            .expect("harvest should succeed");

        assert_eq!(result.theme_id, "TH-1");
        assert_eq!(result.patterns.len(), 2);
        assert_eq!(result.lessons.len(), 1);

        // Theme ID injected into source_theme (when empty)
        assert_eq!(result.patterns[0].source_theme, "TH-1");
        assert_eq!(result.lessons[0].source_theme, "TH-1");

        // Metrics from argument
        assert_eq!(result.metrics.total_jobs, 3);
        assert_eq!(result.metrics.tokens_consumed, Some(15000));

        assert_eq!(result.cross_project_suggestions.len(), 1);
    }

    #[tokio::test]
    async fn test_harvest_with_preamble() {
        let body = format!("Voici les insights :\n\n{}", VALID_RESPONSE);
        let mock = MockLlmClient::with_response(body);
        let brain = InsightsHarvester::new(mock);

        let result = brain
            .harvest("TH-1", "X", "Y", ThemeMetrics::default())
            .await
            .expect("should parse with preamble");
        assert_eq!(result.patterns.len(), 2);
    }

    #[tokio::test]
    async fn test_harvest_empty_response() {
        let mock = MockLlmClient::with_response("".to_string());
        let brain = InsightsHarvester::new(mock);

        let err = brain
            .harvest("TH-1", "X", "Y", ThemeMetrics::default())
            .await
            .expect_err("empty should fail");

        match err {
            InsightsError::ResponseParsingFailed(_) => {}
            other => panic!("wrong: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_harvest_minimal_valid_response() {
        let response = r#"{"patterns": [], "lessons": [], "cross_project_suggestions": [], "summary": "none"}"#;
        let mock = MockLlmClient::with_response(response.to_string());
        let brain = InsightsHarvester::new(mock);

        let result = brain
            .harvest("TH-1", "X", "Y", ThemeMetrics::default())
            .await
            .expect("minimal valid should work");
        assert!(result.patterns.is_empty());
        assert!(result.lessons.is_empty());
        assert_eq!(result.summary, "none");
    }

    #[test]
    fn test_system_prompt_contains_sections() {
        let prompt = build_system_prompt();
        assert!(prompt.contains("patterns"));
        assert!(prompt.contains("lessons"));
        assert!(prompt.contains("cross-projets") || prompt.contains("cross_project"));
        assert!(prompt.contains("quand"));
        assert!(prompt.contains("éviter"));
    }

    #[test]
    fn test_user_prompt_contains_context() {
        let prompt = build_user_prompt("TH-5", "Test theme", "3 jobs done");
        assert!(prompt.contains("TH-5"));
        assert!(prompt.contains("Test theme"));
        assert!(prompt.contains("3 jobs done"));
    }

    #[test]
    fn test_theme_metrics_default() {
        let m = ThemeMetrics::default();
        assert_eq!(m.total_jobs, 0);
        assert_eq!(m.jobs_with_retries, 0);
        assert_eq!(m.avg_retries_per_job, 0.0);
        assert_eq!(m.failure_rate, 0.0);
        assert_eq!(m.tokens_consumed, None);
    }

    #[test]
    fn test_reusable_pattern_serialization() {
        let pattern = ReusablePattern {
            title: "t".into(),
            when: "w".into(),
            then: "th".into(),
            source_theme: "TH-1".into(),
        };
        let json = serde_json::to_string(&pattern).unwrap();
        assert!(json.contains("\"title\":\"t\""));
        let parsed: ReusablePattern = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.when, "w");
    }
}
