//! Architect brain : génération de plans fractals MECE.
//!
//! L'Architecte Fractal prend un objectif utilisateur + contexte Hermes,
//! appelle un LLM avec un prompt structuré, et parse la réponse JSON en
//! structures Rust typées.
//!
//! Le plan résultant est ensuite utilisé par Cortex pour créer les thèmes,
//! jobs, et dépendances dans le Scratchpad.
//!
//! # Principe MECE
//!
//! La décomposition doit être Mutuellement Exclusive, Collectivement Exhaustive :
//! - Mutuellement Exclusive : pas de recouvrement entre thèmes/jobs
//! - Collectivement Exhaustive : objectif couvert à 100%
//!
//! # Criticité
//!
//! Score 1-5 pour chaque thème. Détermine quels brains seront activés ensuite :
//! - 1-2 : pas de Pre-Mortem, pas de Red-Team
//! - 3   : Red-Team obligatoire (critique)
//! - 4-5 : Pre-Mortem + Red-Team + HMAC obligatoire (très critique)

use serde::{Deserialize, Serialize};

use crate::llm_client::{LlmClient, LlmError, LlmRequest};

// ============================================================================
// Types de données du plan fractal
// ============================================================================

/// Plan fractal complet généré par l'Architecte.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FractalPlan {
    pub themes: Vec<PlannedTheme>,
    pub concurrency_groups: Vec<ConcurrencyGroup>,
    pub parking_lot: Vec<ParkingIdea>,
    pub ignored_noise: Vec<IgnoredConstraint>,
    pub impact_warnings: Vec<ImpactWarning>,
}

/// Thème planifié (pas encore ajouté au Scratchpad).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedTheme {
    pub id: String,
    pub name: String,
    pub is_parallel_branch: bool,
    pub convergence_contract: Option<String>,
    pub depends_on: Vec<String>,
    pub criticity_score: u8,
    pub resources_used: Vec<String>,
    pub concurrency_group: Option<String>,
    pub tasks: Vec<PlannedTask>,
}

/// Tâche planifiée à l'intérieur d'un thème.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedTask {
    pub id: String,
    pub name: String,
    pub definition_of_done: String,
    pub depends_on: Vec<String>,
}

/// Groupe de concurrence : ensemble de thèmes qui peuvent s'exécuter en parallèle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConcurrencyGroup {
    pub name: String,
    pub themes: Vec<String>,
    pub execution_mode: String,
    pub reason: String,
}

/// Idée mise en attente (parking lot).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParkingIdea {
    pub idea: String,
    pub priority: String,
}

/// Contrainte filtrée comme bruit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IgnoredConstraint {
    pub constraint: String,
    pub reason: String,
}

/// Avertissement d'impact sur d'autres projets.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImpactWarning {
    pub target_project: String,
    pub impact_type: String,
    pub description: String,
}

// ============================================================================
// Erreurs spécifiques de l'Architect
// ============================================================================

use thiserror::Error;

/// Erreurs spécifiques à la génération de plan par l'Architecte.
#[derive(Error, Debug)]
pub enum ArchitectError {
    #[error("LLM call failed: {0}")]
    LlmCallFailed(#[from] LlmError),
    #[error("LLM response JSON parsing failed: {0}")]
    ResponseParsingFailed(String),
    #[error("Plan validation failed: {0}")]
    PlanValidationFailed(String),
}

// ============================================================================
// Implémentation de l'Architecte
// ============================================================================

/// Cerveau Architecte Fractal.
///
/// Génère un plan complet en appelant un LLM via le trait `LlmClient`.
pub struct Architect<C: LlmClient> {
    client: C,
}

impl<C: LlmClient> Architect<C> {
    /// Crée un Architect avec le client LLM fourni.
    pub fn new(client: C) -> Self {
        Self { client }
    }

    /// Génère un plan fractal pour l'objectif donné.
    ///
    /// # Arguments
    /// * `objective` - Objectif utilisateur (ex: "Refactorise le module auth")
    /// * `context` - Contexte additionnel (contraintes, préférences)
    ///
    /// # Returns
    /// `FractalPlan` structuré avec thèmes, jobs, et analyse d'impacts.
    pub async fn generate_plan(
        &self,
        objective: &str,
        context: &str,
    ) -> Result<FractalPlan, ArchitectError> {
        let system_prompt = build_system_prompt();
        let user_prompt = build_user_prompt(objective, context);

        let request = LlmRequest {
            system: system_prompt,
            user: user_prompt,
            max_tokens: Some(4000),
            temperature: Some(0.3),
            model: None,
        };

        let response = self.client.complete(request).await?;
        let plan = parse_plan_response(&response.text)?;
        validate_plan(&plan)?;

        Ok(plan)
    }
}

// ============================================================================
// Construction du prompt
// ============================================================================

/// Construit le prompt system pour l'Architecte.
fn build_system_prompt() -> String {
    r#"Tu es l'Architecte Fractal, expert en décomposition de systèmes complexes
et en théorie des graphes. Ton rôle est de transformer un objectif macro en un
graphe d'exécution strict, logique et sans chevauchement.

## MÉTHODOLOGIE DE PENSÉE OBLIGATOIRE

Rédige ta réflexion dans une balise <fractal_reasoning> AVANT de donner le JSON.
Ta réflexion doit couvrir :

1. **Analyse du Contexte** : Contraintes passées, décisions d'architecture,
   leçons apprises qui doivent impacter ce plan ?
2. **Principe MECE** : Ma décomposition est-elle Mutuellement Exclusive (pas
   de doublon) et Collectivement Exhaustive (objectif 100% couvert) ?
3. **Granularité** : Chaque tâche est-elle calibrée pour être exécutée en un
   seul "tour" de worker Hermes ? (Ni trop vaste, ni trop micro)
4. **Contrats de Convergence** : Si des branches parallèles fusionnent, quel
   est le "contrat" exact ? (Ex: "TH-3 attend un JSON de TH-1 et un script SQL de TH-2")
5. **Détection d'Impact** : Cette planification impacte-t-elle d'autres projets ?
6. **Auto-Suggestion** : Ai-je oublié une étape de validation, test, ou nettoyage ?

## CONTRAINTES DE CONCURRENCE OBLIGATOIRES

1. Identifie les RESSOURCES PARTAGÉES entre thèmes (fichiers, APIs, DB).
2. Deux thèmes qui touchent la MÊME ressource ne peuvent PAS s'exécuter en parallèle.
3. Limite les thèmes parallèles à MAX 3 pour éviter la surcharge.
4. Pour chaque thème, liste explicitement les ressources qu'il utilise.

## RÈGLES DE GÉNÉRATION DU GRAPHE

- **Dépendances explicites** : Aucune tâche ne commence sans que ses pré-requis
  ne soient définis.
- **Definition of Done (DoD)** : Critères binaires, mesurables, sans ambiguïté.
- **Identification du Parallélisme** : Marque clairement les branches sans
  dépendances entre elles.
- **Criticité (1-5)** :
  - 1-2 : Tâche simple, pas de risque
  - 3   : Tâche moyenne, Red-Team recommandé
  - 4-5 : Tâche critique, Pre-Mortem + Red-Team obligatoires

## FORMAT DE SORTIE OBLIGATOIRE

Commence par <fractal_reasoning>...</fractal_reasoning> puis donne un JSON :

```json
{
  "themes": [
    {
      "id": "TH-1",
      "name": "Nom du thème",
      "is_parallel_branch": true,
      "convergence_contract": "Ce thème doit fournir X au format Y",
      "depends_on": [],
      "criticity_score": 3,
      "resources_used": ["file:config.yaml", "api:example.com"],
      "concurrency_group": "group_A",
      "tasks": [
        {
          "id": "T-1.1",
          "name": "Nom de la tâche",
          "definition_of_done": "Critères binaires vérifiables",
          "depends_on": []
        }
      ]
    }
  ],
  "concurrency_groups": [
    {
      "name": "group_A",
      "themes": ["TH-1", "TH-2"],
      "execution_mode": "parallel",
      "reason": "Pas de ressources partagées"
    }
  ],
  "parking_lot": [
    {"idea": "Idée à explorer plus tard", "priority": "medium"}
  ],
  "ignored_noise": [
    {"constraint": "Contrainte non pertinente", "reason": "Pourquoi on l'ignore"}
  ],
  "impact_warnings": [
    {
      "target_project": "proj_xyz",
      "impact_type": "breaking_change",
      "description": "Impact sur tel projet"
    }
  ]
}
```"#
        .to_string()
}

/// Construit le prompt utilisateur avec l'objectif + contexte.
fn build_user_prompt(objective: &str, context: &str) -> String {
    format!(
        r#"OBJECTIF :
{}

CONTEXTE HERMES :
{}

---

Génère le plan fractal complet pour cet objectif, en respectant la méthodologie
et le format de sortie détaillés dans le système prompt.

Rappelle-toi :
1. Applique le principe MECE strictement
2. Criticité 4-5 = Pre-Mortem obligatoire
3. Chaque tâche = exactement 1 tour de worker
4. Dépendances explicites entre thèmes
"#,
        objective, context
    )
}

// ============================================================================
// Parsing de la réponse LLM
// ============================================================================

/// Parse la réponse du LLM en FractalPlan.
///
/// Gère plusieurs cas :
/// - JSON pur (pas de balises)
/// - JSON entre des balises <fractal_reasoning>...</fractal_reasoning>
/// - JSON après du texte
fn parse_plan_response(response: &str) -> Result<FractalPlan, ArchitectError> {
    // Essaie d'abord de parser la réponse entière
    if let Ok(plan) = serde_json::from_str::<FractalPlan>(response) {
        return Ok(plan);
    }

    // Cherche un bloc JSON dans la réponse (entre { et })
    let json_start = response
        .find('{')
        .ok_or_else(|| ArchitectError::ResponseParsingFailed("No JSON object found".to_string()))?;

    // Trouve l'accolade fermante correspondante (simple heuristique : dernière })
    let json_end = response.rfind('}').ok_or_else(|| {
        ArchitectError::ResponseParsingFailed("No closing braces found".to_string())
    })?;

    if json_end <= json_start {
        return Err(ArchitectError::ResponseParsingFailed(
            "Invalid JSON boundaries".to_string(),
        ));
    }

    let json_str = &response[json_start..=json_end];

    serde_json::from_str::<FractalPlan>(json_str)
        .map_err(|e| ArchitectError::ResponseParsingFailed(format!("JSON parse error: {}", e)))
}

/// Valide le plan généré : vérifications de base.
fn validate_plan(plan: &FractalPlan) -> Result<(), ArchitectError> {
    // Vérifie qu'il y a au moins un thème
    if plan.themes.is_empty() {
        return Err(ArchitectError::PlanValidationFailed(
            "Plan must have at least one theme".to_string(),
        ));
    }

    // Vérifie les IDs uniques
    let mut theme_ids = std::collections::HashSet::new();
    for theme in &plan.themes {
        if !theme_ids.insert(&theme.id) {
            return Err(ArchitectError::PlanValidationFailed(format!(
                "Duplicate theme ID: {}",
                theme.id
            )));
        }

        // Vérifie criticité entre 1 et 5
        if theme.criticity_score < 1 || theme.criticity_score > 5 {
            return Err(ArchitectError::PlanValidationFailed(format!(
                "Theme {} criticity must be 1-5, got {}",
                theme.id, theme.criticity_score
            )));
        }

        if theme.tasks.is_empty() {
            return Err(ArchitectError::PlanValidationFailed(format!(
                "Theme {} must have at least one task",
                theme.id
            )));
        }
    }

    // Vérifie que les dépendances pointent vers des thèmes existants
    for theme in &plan.themes {
        for dep in &theme.depends_on {
            if !theme_ids.contains(dep) {
                return Err(ArchitectError::PlanValidationFailed(format!(
                    "Theme {} depends on non-existent theme {}",
                    theme.id, dep
                )));
            }
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

    const VALID_PLAN_JSON: &str = r#"{
        "themes": [
            {
                "id": "TH-1",
                "name": "Audit code actuel",
                "is_parallel_branch": true,
                "convergence_contract": "JSON des dépendances + métriques",
                "depends_on": [],
                "criticity_score": 3,
                "resources_used": ["file:src/auth/"],
                "concurrency_group": "group_A",
                "tasks": [
                    {
                        "id": "T-1.1",
                        "name": "Scan dépendances",
                        "definition_of_done": "JSON avec au moins 10 dépendances",
                        "depends_on": []
                    },
                    {
                        "id": "T-1.2",
                        "name": "Métriques qualité",
                        "definition_of_done": "Rapport HTML avec coverage > 80%",
                        "depends_on": []
                    }
                ]
            },
            {
                "id": "TH-2",
                "name": "Research JWT modernes",
                "is_parallel_branch": true,
                "convergence_contract": "Liste bibliothèques recommandées",
                "depends_on": [],
                "criticity_score": 2,
                "resources_used": ["web"],
                "concurrency_group": "group_A",
                "tasks": [
                    {
                        "id": "T-2.1",
                        "name": "Comparaison PyJWT vs python-jose",
                        "definition_of_done": "Tableau comparatif avec au moins 5 critères",
                        "depends_on": []
                    }
                ]
            },
            {
                "id": "TH-3",
                "name": "Implémentation migration",
                "is_parallel_branch": false,
                "convergence_contract": null,
                "depends_on": ["TH-1", "TH-2"],
                "criticity_score": 5,
                "resources_used": ["file:src/auth/", "db:users"],
                "concurrency_group": "group_B",
                "tasks": [
                    {
                        "id": "T-3.1",
                        "name": "Migration tables auth",
                        "definition_of_done": "Toutes migrations appliquées sans erreurs",
                        "depends_on": []
                    }
                ]
            }
        ],
        "concurrency_groups": [
            {
                "name": "group_A",
                "themes": ["TH-1", "TH-2"],
                "execution_mode": "parallel",
                "reason": "Pas de ressources partagées"
            },
            {
                "name": "group_B",
                "themes": ["TH-3"],
                "execution_mode": "sequential_after_group_A",
                "reason": "TH-3 dépend de TH-1 et TH-2"
            }
        ],
        "parking_lot": [
            {"idea": "Ajouter monitoring Prometheus", "priority": "low"}
        ],
        "ignored_noise": [
            {"constraint": "Utiliser PHP 4", "reason": "Incompatible Python stack"}
        ],
        "impact_warnings": [
            {
                "target_project": "proj_mobile",
                "impact_type": "shared_dependency",
                "description": "proj_mobile utilise aussi db:users, coordination requise"
            }
        ]
    }"#;

    #[tokio::test]
    async fn test_architect_generates_plan_with_mock() {
        let mock = MockLlmClient::with_response(VALID_PLAN_JSON.to_string());
        let architect = Architect::new(mock);

        let plan = architect
            .generate_plan(
                "Refactorise le module auth",
                "Contexte FastAPI + PostgreSQL",
            )
            .await
            .expect("plan generation should succeed");

        assert_eq!(plan.themes.len(), 3);

        // Vérifie TH-1 (critique, parallel)
        let th1 = &plan.themes[0];
        assert_eq!(th1.id, "TH-1");
        assert_eq!(th1.criticity_score, 3);
        assert!(th1.is_parallel_branch);
        assert_eq!(th1.tasks.len(), 2);
        assert!(th1.depends_on.is_empty());

        // Vérifie TH-3 dépend de TH-1 et TH-2
        let th3 = &plan.themes[2];
        assert_eq!(th3.id, "TH-3");
        assert_eq!(th3.criticity_score, 5);
        assert!(th3.depends_on.contains(&"TH-1".to_string()));
        assert!(th3.depends_on.contains(&"TH-2".to_string()));

        // Vérifie groupes de concurrence
        assert_eq!(plan.concurrency_groups.len(), 2);
        assert_eq!(plan.concurrency_groups[0].themes.len(), 2);

        // Vérifie parking lot
        assert_eq!(plan.parking_lot.len(), 1);

        // Vérifie impact warnings
        assert_eq!(plan.impact_warnings.len(), 1);

        // Vérifie ignored noise
        assert_eq!(plan.ignored_noise.len(), 1);
    }

    #[tokio::test]
    async fn test_architect_parses_json_with_preamble() {
        // LLM retourne parfois du texte avant le JSON (ex: <fractal_reasoning>)
        let response_with_preamble = format!(
            "<fractal_reasoning>Blabla de réflexion...</fractal_reasoning>\n\nVoici le plan :\n{}",
            VALID_PLAN_JSON
        );
        let mock = MockLlmClient::with_response(response_with_preamble);
        let architect = Architect::new(mock);

        let plan = architect
            .generate_plan("Objectif", "Contexte")
            .await
            .expect("should parse JSON from preamble");

        assert_eq!(plan.themes.len(), 3);
    }

    #[tokio::test]
    async fn test_architect_fails_on_invalid_json() {
        let mock = MockLlmClient::with_response("Pas un JSON valide".to_string());
        let architect = Architect::new(mock);

        let err = architect
            .generate_plan("Objectif", "Contexte")
            .await
            .expect_err("should fail on invalid JSON");

        match err {
            ArchitectError::ResponseParsingFailed(_) => {} // OK
            other => panic!("unexpected error: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_architect_fails_on_empty_themes() {
        let empty_plan = r#"{"themes": [], "concurrency_groups": [], "parking_lot": [], "ignored_noise": [], "impact_warnings": []}"#;
        let mock = MockLlmClient::with_response(empty_plan.to_string());
        let architect = Architect::new(mock);

        let err = architect
            .generate_plan("Objectif", "Contexte")
            .await
            .expect_err("should fail on empty themes");

        match err {
            ArchitectError::PlanValidationFailed(_) => {} // OK
            other => panic!("unexpected error: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_architect_fails_on_duplicate_theme_id() {
        let duplicate = r#"{
            "themes": [
                {"id": "TH-1", "name": "A", "is_parallel_branch": true, "convergence_contract": null, "depends_on": [], "criticity_score": 3, "resources_used": [], "concurrency_group": null, "tasks": [{"id": "T-1.1", "name": "T", "definition_of_done": "D", "depends_on": []}]},
                {"id": "TH-1", "name": "B", "is_parallel_branch": true, "convergence_contract": null, "depends_on": [], "criticity_score": 3, "resources_used": [], "concurrency_group": null, "tasks": [{"id": "T-2.1", "name": "T", "definition_of_done": "D", "depends_on": []}]}
            ],
            "concurrency_groups": [],
            "parking_lot": [],
            "ignored_noise": [],
            "impact_warnings": []
        }"#;
        let mock = MockLlmClient::with_response(duplicate.to_string());
        let architect = Architect::new(mock);

        let err = architect
            .generate_plan("Objectif", "Contexte")
            .await
            .expect_err("should fail on duplicate theme id");

        match err {
            ArchitectError::PlanValidationFailed(msg) => {
                assert!(msg.contains("Duplicate theme ID"));
            }
            other => panic!("unexpected error: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_architect_fails_on_invalid_criticity() {
        let invalid_criticity = r#"{
            "themes": [
                {"id": "TH-1", "name": "A", "is_parallel_branch": true, "convergence_contract": null, "depends_on": [], "criticity_score": 7, "resources_used": [], "concurrency_group": null, "tasks": [{"id": "T-1.1", "name": "T", "definition_of_done": "D", "depends_on": []}]}
            ],
            "concurrency_groups": [],
            "parking_lot": [],
            "ignored_noise": [],
            "impact_warnings": []
        }"#;
        let mock = MockLlmClient::with_response(invalid_criticity.to_string());
        let architect = Architect::new(mock);

        let err = architect
            .generate_plan("Objectif", "Contexte")
            .await
            .expect_err("should fail on invalid criticity");

        match err {
            ArchitectError::PlanValidationFailed(msg) => {
                assert!(msg.contains("criticity"));
            }
            other => panic!("unexpected error: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_architect_fails_on_bad_dependency() {
        let bad_dep = r#"{
            "themes": [
                {"id": "TH-1", "name": "A", "is_parallel_branch": true, "convergence_contract": null, "depends_on": ["TH-999"], "criticity_score": 3, "resources_used": [], "concurrency_group": null, "tasks": [{"id": "T-1.1", "name": "T", "definition_of_done": "D", "depends_on": []}]}
            ],
            "concurrency_groups": [],
            "parking_lot": [],
            "ignored_noise": [],
            "impact_warnings": []
        }"#;
        let mock = MockLlmClient::with_response(bad_dep.to_string());
        let architect = Architect::new(mock);

        let err = architect
            .generate_plan("Objectif", "Contexte")
            .await
            .expect_err("should fail on bad dependency");

        match err {
            ArchitectError::PlanValidationFailed(msg) => {
                assert!(msg.contains("non-existent theme"));
            }
            other => panic!("unexpected error: {:?}", other),
        }
    }

    #[test]
    fn test_build_system_prompt_contains_key_sections() {
        let prompt = build_system_prompt();
        assert!(prompt.contains("Architecte Fractal"));
        assert!(prompt.contains("MECE"));
        assert!(prompt.contains("criticity_score"));
        assert!(prompt.contains("fractal_reasoning"));
        assert!(prompt.contains("convergence_contract"));
    }

    #[test]
    fn test_build_user_prompt_includes_objective_and_context() {
        let prompt = build_user_prompt("Mon objectif", "Mon contexte");
        assert!(prompt.contains("Mon objectif"));
        assert!(prompt.contains("Mon contexte"));
    }
}
