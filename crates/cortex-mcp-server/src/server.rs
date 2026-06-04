//! Cortex Server : cœur métier du système.
//!
//! `CortexServer` encapsule tous les composants nécessaires au fonctionnement
//! de Cortex : WAL, Architect brain, Actor registry, routing rules.
//!
//! Cette struct expose les 7 outils MCP sous forme de méthodes async.
//! La couche transport (rmcp) appelle ces méthodes.

use std::sync::Arc;

use cortex_brains::{Architect, FractalPlan, LlmClient};
use cortex_core::{RoutingRules, WalService};

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ============================================================================
// Request / Response types (format MCP JSON-RPC)
// ============================================================================

/// Requête pour l'outil `intercept_plan`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterceptPlanRequest {
    /// Intention brute de l'utilisateur (texte libre).
    pub intent: String,
    /// Contexte additionnel fourni par Hermes (contraintes, historique, etc.).
    pub context: String,
    /// ID projet si existant, sinon None.
    pub project_id: Option<String>,
}

/// Réponse de l'outil `intercept_plan`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InterceptPlanResponse {
    /// ID du projet (nouveau ou existant).
    pub project_id: String,
    /// Plan fractal généré par l'Architecte.
    pub plan: FractalPlan,
    /// Si l'utilisateur doit approuver avant approbation_and_execute.
    pub requires_user_approval: bool,
    /// Résumé pour affichage user-friendly.
    pub summary_for_user: String,
}

/// Réponse de l'outil `get_routing_rules`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GetRoutingRulesResponse {
    pub rules: RoutingRules,
}

/// Requête pour l'outil `pre_mortem`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreMortemRequest {
    pub job_id: String,
    pub job_description: String,
    pub definition_of_done: String,
    #[serde(default)]
    pub context: String,
}

/// Réponse de l'outil `pre_mortem`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreMortemResponse {
    pub job_id: String,
    pub guardrails: Vec<cortex_brains::paranoiac::ExecutableGuardrail>,
    pub risk_assessment: String,
    pub estimated_risk_score: u8,
}

/// Requête pour l'outil `red_team_audit`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedTeamAuditRequest {
    pub job_id: String,
    pub definition_of_done: String,
    #[serde(default)]
    pub convergence_contract: Option<String>,
    #[serde(default)]
    pub guardrails: Vec<cortex_brains::paranoiac::ExecutableGuardrail>,
    pub artifact: String,
}

/// Réponse de l'outil `red_team_audit`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedTeamAuditResponse {
    pub job_id: String,
    pub passed: bool,
    pub issues: Vec<cortex_brains::red_team::AuditIssue>,
    pub approved_layers: Vec<String>,
    pub summary: String,
}

/// Requête pour l'outil `harvest_insights`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarvestInsightsRequest {
    pub theme_id: String,
    pub theme_name: String,
    pub jobs_summary: String,
    #[serde(default)]
    pub metrics: cortex_brains::insights::ThemeMetrics,
}

/// Réponse de l'outil `harvest_insights`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarvestInsightsResponse {
    pub insights: cortex_brains::insights::InsightsResult,
}

// ============================================================================
// Erreurs spécifiques au serveur Cortex
// ============================================================================

/// Erreurs métier du serveur Cortex (distinctes des erreurs MCP transport).
#[derive(Error, Debug)]
pub enum CortexServerError {
    #[error("Intercept plan failed: {0}")]
    InterceptPlanFailed(String),

    #[error("Project not found: {0}")]
    ProjectNotFound(String),

    #[error("WAL error: {0}")]
    WalError(String),

    #[error("Validation error: {0}")]
    ValidationError(String),

    #[error("Internal error: {0}")]
    InternalError(String),
}

impl From<cortex_core::CortexError> for CortexServerError {
    fn from(e: cortex_core::CortexError) -> Self {
        CortexServerError::WalError(e.to_string())
    }
}

impl From<cortex_brains::ArchitectError> for CortexServerError {
    fn from(e: cortex_brains::ArchitectError) -> Self {
        CortexServerError::InterceptPlanFailed(e.to_string())
    }
}

impl From<cortex_brains::paranoiac::PreMortemError> for CortexServerError {
    fn from(e: cortex_brains::paranoiac::PreMortemError) -> Self {
        CortexServerError::InternalError(format!("PreMortem failed: {}", e))
    }
}

impl From<cortex_brains::red_team::RedTeamError> for CortexServerError {
    fn from(e: cortex_brains::red_team::RedTeamError) -> Self {
        CortexServerError::InternalError(format!("RedTeam failed: {}", e))
    }
}

impl From<cortex_brains::insights::InsightsError> for CortexServerError {
    fn from(e: cortex_brains::insights::InsightsError) -> Self {
        CortexServerError::InternalError(format!("Insights failed: {}", e))
    }
}

// ============================================================================
// CortexServer struct
// ============================================================================

/// Serveur Cortex : expose les outils MCP métier.
///
/// # Generic Parameter
///
/// `C` : type du LlmClient injecté.
/// - `MockLlmClient` pour les tests
/// - Implémentation HTTP (reqwest) ou autre en production
pub struct CortexServer<C: LlmClient + Clone> {
    wal: Arc<WalService>,
    llm_client: C,
    architect: Architect<C>,
    routing_rules: RoutingRules,
}

impl<C: LlmClient + Clone> CortexServer<C> {
    /// Crée une instance du serveur Cortex.
    pub fn new(wal: WalService, architect: Architect<C>, llm_client: C) -> Self {
        Self {
            wal: Arc::new(wal),
            llm_client,
            architect,
            routing_rules: RoutingRules::default_rules(),
        }
    }

    /// Crée une instance avec des routing_rules custom (pour tests).
    pub fn with_routing_rules(
        wal: WalService,
        architect: Architect<C>,
        llm_client: C,
        routing_rules: RoutingRules,
    ) -> Self {
        Self {
            wal: Arc::new(wal),
            llm_client,
            architect,
            routing_rules,
        }
    }

    /// Outil `get_routing_rules` : retourne les règles de routage pour Hermes.
    ///
    /// Appelé par Hermes au boot pour configurer son système de routage.
    pub async fn get_routing_rules(&self) -> Result<GetRoutingRulesResponse, CortexServerError> {
        Ok(GetRoutingRulesResponse {
            rules: self.routing_rules.clone(),
        })
    }

    /// Outil `intercept_plan` : génère un plan fractal pour l'intention donnée.
    ///
    /// Workflow :
    /// 1. Log la requête dans WAL (prepare)
    /// 2. Appelle l'Architect brain
    /// 3. Log la réponse dans WAL (commit)
    /// 4. Retourne le plan + metadata
    pub async fn intercept_plan(
        &self,
        request: InterceptPlanRequest,
    ) -> Result<InterceptPlanResponse, CortexServerError> {
        // 1. Détermine project_id (nouveau ou réutilise)
        let project_id = match request.project_id {
            Some(id) => id,
            None => cortex_core::generate_project_id(),
        };

        // 2. WAL prepare : log la demande
        let entry_id = self
            .wal
            .write_prepare(
                &project_id,
                "intercept_plan_request",
                None,
                None,
                &serde_json::json!({
                    "intent": request.intent,
                    "context": request.context
                }),
            )
            .await?;

        // 3. Appelle l'Architect pour générer le plan
        let plan = self
            .architect
            .generate_plan(&request.intent, &request.context)
            .await?;

        // 4. Génère résumé user-friendly
        let summary = build_summary(&plan);

        // 5. WAL commit : enregistre la réponse
        self.wal.write_commit(&entry_id).await?;

        // 6. WAL snapshot : sauvegarde du plan comme commit historique
        self.wal
            .write_snapshot_commit(
                &project_id,
                "plan_generated",
                "intercept_plan_request",
                &serde_json::json!({"plan_summary": summary}),
                &serde_json::json!({"plan": plan}),
            )
            .await?;

        Ok(InterceptPlanResponse {
            project_id,
            plan,
            requires_user_approval: true,
            summary_for_user: summary,
        })
    }

    /// Détermine si une intention devrait être routée vers Cortex.
    ///
    /// Utilise les règles de routage chargées au boot.
    pub fn should_route(&self, intent: &str, complexity: u8) -> bool {
        self.routing_rules
            .should_route_to_cortex(intent, complexity)
    }

    /// Outil `pre_mortem` : génère des guardrails pour un job.
    ///
    /// Utilisé par Hermes avant dispatch d'un job à criticité ≥ 4.
    pub async fn pre_mortem(
        &self,
        request: PreMortemRequest,
    ) -> Result<PreMortemResponse, CortexServerError> {
        let brain = cortex_brains::PreMortem::new(self.llm_client.clone());
        let result = brain
            .generate_guardrails(
                &request.job_id,
                &request.job_description,
                &request.definition_of_done,
                &request.context,
            )
            .await?;

        Ok(PreMortemResponse {
            job_id: result.job_id,
            guardrails: result.guardrails,
            risk_assessment: result.risk_assessment,
            estimated_risk_score: result.estimated_risk_score,
        })
    }

    /// Outil `red_team_audit` : audit adversarial d'un artéfact worker.
    pub async fn red_team_audit(
        &self,
        request: RedTeamAuditRequest,
    ) -> Result<RedTeamAuditResponse, CortexServerError> {
        let brain = cortex_brains::RedTeam::new(self.llm_client.clone());
        let result = brain
            .audit(
                &request.job_id,
                &request.definition_of_done,
                request.convergence_contract.as_deref(),
                &request.guardrails,
                &request.artifact,
            )
            .await?;

        Ok(RedTeamAuditResponse {
            job_id: result.job_id,
            passed: result.passed,
            issues: result.issues,
            approved_layers: result.approved_layers,
            summary: result.summary,
        })
    }

    /// Outil `harvest_insights` : extrait patterns/leçons d'un thème complété.
    pub async fn harvest_insights(
        &self,
        request: HarvestInsightsRequest,
    ) -> Result<HarvestInsightsResponse, CortexServerError> {
        let brain = cortex_brains::InsightsHarvester::new(self.llm_client.clone());
        let insights = brain
            .harvest(
                &request.theme_id,
                &request.theme_name,
                &request.jobs_summary,
                request.metrics,
            )
            .await?;

        Ok(HarvestInsightsResponse { insights })
    }
    /// Référence au WAL service (pour usage avancé, ex: tools.rs).
    pub fn wal(&self) -> &WalService {
        &self.wal
    }
}

// ============================================================================
// Helpers
// ============================================================================

/// Génère un résumé user-friendly du plan.
fn build_summary(plan: &FractalPlan) -> String {
    let themes_count = plan.themes.len();
    let tasks_count: usize = plan.themes.iter().map(|t| t.tasks.len()).sum();
    let parallel_count = plan.themes.iter().filter(|t| t.is_parallel_branch).count();
    let max_criticity = plan
        .themes
        .iter()
        .map(|t| t.criticity_score)
        .max()
        .unwrap_or(0);

    let mut parts = vec![format!(
        "{} thème{} avec {} tâche{} total",
        themes_count,
        if themes_count > 1 { "s" } else { "" },
        tasks_count,
        if tasks_count > 1 { "s" } else { "" }
    )];

    if parallel_count > 0 {
        parts.push(format!("{} en parallèle", parallel_count));
    }

    if max_criticity >= 4 {
        parts.push(format!("criticité max {} (Pre-Mortem activé)", max_criticity));
    } else if max_criticity >= 3 {
        parts.push(format!("criticité max {} (Red-Team activé)", max_criticity));
    }

    if !plan.impact_warnings.is_empty() {
        parts.push(format!(
            "⚠️  {} impact{} inter-projet{}",
            plan.impact_warnings.len(),
            if plan.impact_warnings.len() > 1 {
                "s"
            } else {
                ""
            },
            if plan.impact_warnings.len() > 1 {
                "s"
            } else {
                ""
            }
        ));
    }

    if !plan.parking_lot.is_empty() {
        parts.push(format!(
            "{} idée{} en attente",
            plan.parking_lot.len(),
            if plan.parking_lot.len() > 1 { "s" } else { "" }
        ));
    }

    parts.join(" · ")
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use cortex_brains::MockLlmClient;
    use cortex_core::WalService as TestedWalService;

    const VALID_PLAN_JSON: &str = r#"{
        "themes": [
            {
                "id": "TH-1",
                "name": "Audit code",
                "is_parallel_branch": true,
                "convergence_contract": null,
                "depends_on": [],
                "criticity_score": 3,
                "resources_used": ["src/"],
                "concurrency_group": "group_A",
                "tasks": [
                    {"id": "T-1.1", "name": "Scan deps", "definition_of_done": "List", "depends_on": []}
                ]
            },
            {
                "id": "TH-2",
                "name": "Impl migration",
                "is_parallel_branch": false,
                "convergence_contract": null,
                "depends_on": ["TH-1"],
                "criticity_score": 5,
                "resources_used": ["src/", "db"],
                "concurrency_group": "group_B",
                "tasks": [
                    {"id": "T-2.1", "name": "Migrate", "definition_of_done": "OK", "depends_on": []}
                ]
            }
        ],
        "concurrency_groups": [],
        "parking_lot": [
            {"idea": "Monitoring", "priority": "low"}
        ],
        "ignored_noise": [],
        "impact_warnings": [
            {"target_project": "proj_mobile", "impact_type": "shared_dep", "description": "db"}
        ]
    }"#;

    #[tokio::test]
    async fn test_cortex_server_new() {
        let wal = TestedWalService::connect("sqlite::memory:")
            .await
            .expect("in-memory WAL");
        let mock = MockLlmClient::with_response(VALID_PLAN_JSON.to_string());
        let server = CortexServer::new(wal, Architect::new(mock.clone()), mock);

        // Server should have default rules
        let resp = server.get_routing_rules().await.expect("rules");
        assert_eq!(resp.rules.version, "1.0");
    }

    #[tokio::test]
    async fn test_get_routing_rules_returns_default() {
        let wal = TestedWalService::connect("sqlite::memory:").await.unwrap();
        let mock = MockLlmClient::with_response("".to_string());
        let server = CortexServer::new(wal, Architect::new(mock.clone()), mock);

        let resp = server.get_routing_rules().await.unwrap();
        assert!(resp.rules.enabled);
        assert!(!resp.rules.intent_patterns.is_empty());
    }

    #[tokio::test]
    async fn test_intercept_plan_generates_plan() {
        let wal = TestedWalService::connect("sqlite::memory:").await.unwrap();
        let mock = MockLlmClient::with_response(VALID_PLAN_JSON.to_string());
        let server = CortexServer::new(wal, Architect::new(mock.clone()), mock);

        let req = InterceptPlanRequest {
            intent: "Refactorise le module auth".to_string(),
            context: "Projet FastAPI + PostgreSQL".to_string(),
            project_id: None,
        };

        let resp = server.intercept_plan(req).await.expect("plan should work");

        // Plan généré
        assert_eq!(resp.plan.themes.len(), 2);
        assert!(resp.requires_user_approval);

        // Nouveau project_id attribué
        assert!(resp.project_id.starts_with("project-"));

        // Résumé généré
        assert!(resp.summary_for_user.contains("2 thèmes"));
        assert!(resp.summary_for_user.contains("criticité max 5"));
    }

    #[tokio::test]
    async fn test_intercept_plan_uses_provided_project_id() {
        let wal = TestedWalService::connect("sqlite::memory:").await.unwrap();
        let mock = MockLlmClient::with_response(VALID_PLAN_JSON.to_string());
        let server = CortexServer::new(wal, Architect::new(mock.clone()), mock);

        let req = InterceptPlanRequest {
            intent: "Refactorise le module".to_string(),
            context: "".to_string(),
            project_id: Some("proj_existing_123".to_string()),
        };

        let resp = server.intercept_plan(req).await.unwrap();
        assert_eq!(resp.project_id, "proj_existing_123");
    }

    #[tokio::test]
    async fn test_intercept_plan_logs_to_wal() {
        let wal = TestedWalService::connect("sqlite::memory:").await.unwrap();
        let mock = MockLlmClient::with_response(VALID_PLAN_JSON.to_string());
        let server = CortexServer::new(wal, Architect::new(mock.clone()), mock);

        let req = InterceptPlanRequest {
            intent: "Refactorise".to_string(),
            context: "".to_string(),
            project_id: Some("proj_1".to_string()),
        };

        server.intercept_plan(req).await.unwrap();

        // Vérifie que WAL a un commit pour le projet
        let commits = server.wal().list_commits("proj_1").await.unwrap();
        assert!(!commits.is_empty());
        assert_eq!(commits[0].mutation_type, "plan_generated");
    }

    #[tokio::test]
    async fn test_intercept_plan_fails_on_invalid_llm_response() {
        let wal = TestedWalService::connect("sqlite::memory:").await.unwrap();
        let mock = MockLlmClient::with_response("pas un JSON valide".to_string());
        let server = CortexServer::new(wal, Architect::new(mock.clone()), mock);

        let req = InterceptPlanRequest {
            intent: "Refactorise".to_string(),
            context: "".to_string(),
            project_id: Some("proj_1".to_string()),
        };

        let err = server
            .intercept_plan(req)
            .await
            .expect_err("should fail on invalid JSON");

        match err {
            CortexServerError::InterceptPlanFailed(msg) => {
                assert!(msg.contains("parse") || msg.contains("JSON"));
            }
            other => panic!("unexpected error: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_should_route_uses_routing_rules() {
        let wal = TestedWalService::connect("sqlite::memory:").await.unwrap();
        let mock = MockLlmClient::with_response("".to_string());
        let server = CortexServer::new(wal, Architect::new(mock.clone()), mock);

        // Intent avec "refactor" + complexity ≥ 5 → route
        assert!(server.should_route("Refactorise le module", 7));

        // Complexity trop basse → no route
        assert!(!server.should_route("Refactorise le module", 3));

        // Intent sans mot-clé → no route
        assert!(!server.should_route("Fix this bug", 8));
    }

    #[test]
    fn test_build_summary_parallel_themes() {
        let plan: FractalPlan = serde_json::from_str(VALID_PLAN_JSON).unwrap();
        let summary = build_summary(&plan);

        assert!(summary.contains("2 thèmes"));
        assert!(summary.contains("en parallèle"));
        assert!(summary.contains("criticité max 5"));
        assert!(summary.contains("inter-projet"));
    }

    #[test]
    fn test_build_summary_no_parallel() {
        // Cas minimal : 1 thème non-parallèle
        let json = r#"{
            "themes": [{
                "id": "TH-1", "name": "X", "is_parallel_branch": false,
                "convergence_contract": null, "depends_on": [], "criticity_score": 2,
                "resources_used": [], "concurrency_group": null,
                "tasks": [{"id": "T-1.1", "name": "T", "definition_of_done": "D", "depends_on": []}]
            }],
            "concurrency_groups": [], "parking_lot": [], "ignored_noise": [], "impact_warnings": []
        }"#;
        let plan: FractalPlan = serde_json::from_str(json).unwrap();
        let summary = build_summary(&plan);

        assert!(summary.contains("1 thème"));
        assert!(!summary.contains("en parallèle"));
        assert!(!summary.contains("Pre-Mortem"));
        assert!(!summary.contains("Red-Team"));
    }
}
