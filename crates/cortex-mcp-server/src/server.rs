//! Cortex Server : cœur métier du système.
//!
//! `CortexServer` encapsule tous les composants nécessaires au fonctionnement
//! de Cortex : WAL, Architect brain, Actor registry, routing rules.
//!
//! Cette struct expose les 7 outils MCP sous forme de méthodes async.
//! La couche transport (rmcp) appelle ces méthodes.

use std::sync::Arc;

use cortex_brains::{Architect, FractalPlan, LlmClient};
use cortex_core::{metrics::HistogramExt, RecoveryReport, RoutingRules, SharedMetrics, WalService};
use cortex_actors::{ActorRegistry, ProjectActorHandle, JobResult, AuditRecord};

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
    /// Signature HMAC-SHA256 des guardrails (calculée par Cortex Pre-Mortem).
    /// Si présente et que le serveur a un secret HMAC, elle est vérifiée
    /// (couche 1 d'audit, anti-tampering workers).
    #[serde(default)]
    pub guardrails_signature: Option<String>,
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

// =============================================================================
// Worker lifecycle tools (Steps 2-6 of Session 4)
// =============================================================================

/// Requête pour l'outil `approve_and_execute`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApproveAndExecuteRequest {
    pub project_id: String,
    pub approved_by: String,
    #[serde(default)]
    pub plan: Option<cortex_brains::FractalPlan>,
}

/// Un thème prêt à être dispatché (avec ses dépendances résolues).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchableTheme {
    pub theme_id: String,
    pub theme_name: String,
    pub criticity_score: u8,
    pub estimated_tokens: u32,
    pub depends_on: Vec<String>,
    pub guardrails_request: bool, // criticity ≥ 4
    pub red_team_audit_request: bool, // criticity ≥ 3
    pub tasks: Vec<cortex_brains::PlannedTask>,
}

/// Ordre de dispatch complet retourné à Hermes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchOrder {
    pub project_id: String,
    pub phases: Vec<Vec<DispatchableTheme>>, // chaque phase = themes parallélisables
    pub total_themes: u32,
    pub total_estimated_tokens: u32,
    pub cost_gating_summary: String,
}

/// Réponse de l'outil `approve_and_execute`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApproveAndExecuteResponse {
    pub dispatch_order: DispatchOrder,
    pub instructions: String, // Markdown lisible par user
}

/// Requête pour l'outil `sync_reflect`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncReflectRequest {
    pub project_id: String,
    pub job_id: String,
    pub artifact: String,
    pub definition_of_done: String,
    #[serde(default)]
    pub convergence_contract: Option<String>,
    /// Signature HMAC-SHA256 des guardrails (forwarded to red_team_audit).
    #[serde(default)]
    pub guardrails_signature: Option<String>,
}

/// Réponse de l'outil `sync_reflect`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncReflectResponse {
pub job_id: String,
pub passed: bool,
pub approved: bool, // = passed
pub action: String, // "commit" | "retry" | "escalate"
pub audit: RedTeamAuditResponse,
}

/// Requête pour l'outil `check_jobs_status`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckJobsStatusRequest {
    pub project_id: String,
}

/// Status d'un thème.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThemeStatus {
    pub theme_id: String,
    pub name: String,
    pub criticity_score: u8,
    pub tasks_count: u32,
    pub jobs_status: String, // "pending" | "running" | "completed" | "failed"
}

/// Réponse de l'outil `check_jobs_status`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckJobsStatusResponse {
    pub project_id: String,
    pub total_commits: u32,
    pub last_commit_at: Option<i64>,
    pub themes: Vec<ThemeStatus>,
    pub handoff_summary: Option<String>,
}

/// Requête pour l'outil `rollback`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollbackRequest {
    pub project_id: String,
    /// ID du commit cible (default: previous commit)
    #[serde(default)]
    pub target_commit_id: Option<String>,
    pub reason: String,
}

/// Réponse de l'outil `rollback`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RollbackResponse {
    pub project_id: String,
    pub rolled_back_from: String,
    pub rolled_back_to: String,
    pub restored_state: serde_json::Value,
}

/// Requête pour l'outil `abort`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbortRequest {
    pub project_id: String,
    pub reason: String,
}

/// Réponse de l'outil `abort`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbortResponse {
    pub project_id: String,
    pub aborted: bool,
    pub reason: String,
    pub aborted_at: i64,
}

/// Requête pour l'outil `recover_project` (crash recovery).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoverProjectRequest {
    pub project_id: String,
}

/// Réponse de l'outil `recover_project` — utilise `cortex_core::RecoveryReport`.
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
    /// HMAC-SHA256 secret pour signer les guardrails Pre-Mortem.
    /// None = pas de signature (workers peuvent altérer les guardrails).
    pub hmac_secret: Option<Vec<u8>>,
    /// Compteurs Prometheus partagés (lock-free).
    pub metrics: SharedMetrics,
    /// Actor registry (Session 6, option C).
    /// Chaque project_id a son ProjectActor qui owns son state.
    /// Race-free, async-first, pas de Mutex.
    pub actor_registry: Arc<ActorRegistry>,
}

impl<C: LlmClient + Clone> CortexServer<C> {
    /// Crée une instance du serveur Cortex.
    pub fn new(wal: WalService, architect: Architect<C>, llm_client: C) -> Self {
        Self::with_metrics(wal, architect, llm_client, Arc::new(cortex_core::Metrics::new()))
    }

    /// Constructeur qui injecte des metrics custom (utile pour tests).
    pub fn with_metrics(
        wal: WalService,
        architect: Architect<C>,
        llm_client: C,
        metrics: SharedMetrics,
    ) -> Self {
        Self::with_metrics_and_registry(
            wal,
            architect,
            llm_client,
            metrics,
            Arc::new(ActorRegistry::new()),
        )
    }

    /// Constructeur complet (utilisé par `with_metrics` et les tests).
    pub fn with_metrics_and_registry(
        wal: WalService,
        architect: Architect<C>,
        llm_client: C,
        metrics: SharedMetrics,
        actor_registry: Arc<ActorRegistry>,
    ) -> Self {
        Self {
            wal: Arc::new(wal),
            llm_client,
            architect,
            routing_rules: RoutingRules::default_rules(),
            hmac_secret: None,
            metrics,
            actor_registry,
        }
    }

    /// Helper : get-or-spawn un actor pour un project_id.
    /// Utilisé par tous les handlers project-scoped (intercept_plan, abort, etc.)
    async fn get_or_spawn_actor(
        &self,
        project_id: &str,
    ) -> Result<ProjectActorHandle, CortexServerError> {
        // Default project_name/objective : on les enrichira via les messages.
        // L'actor ne s'en sert que pour le scratchpad initial.
        self.actor_registry
            .get_or_create(project_id, project_id, "")
            .map_err(|e| CortexServerError::InternalError(format!("actor registry: {}", e)))
    }

    /// Configure le secret HMAC pour signer les guardrails Pre-Mortem.
    ///
    /// Si `secret` est vide ou None, le signing est désactivé.
    pub fn with_hmac_secret(mut self, secret: Option<&str>) -> Self {
        self.hmac_secret = secret
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::as_bytes)
            .map(Vec::from);
        self
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
            hmac_secret: None,
            metrics: Arc::new(cortex_core::Metrics::new()),
            actor_registry: Arc::new(ActorRegistry::new()),
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
    /// 1. Get-or-spawn le ProjectActor (option C, Session 6)
    /// 2. Court-circuit si le projet est aborted
    /// 3. Log la requête dans WAL (prepare)
    /// 4. Appelle l'Architect brain
    /// 5. Log la réponse dans WAL (commit)
    /// 6. Enregistre le plan dans l'actor (state ownership)
    /// 7. WAL snapshot : sauvegarde du plan
    pub async fn intercept_plan(
        &self,
        request: InterceptPlanRequest,
    ) -> Result<InterceptPlanResponse, CortexServerError> {
        // Session 6 (option A) : démarre le timer pour histogramme Prometheus.
        // Le timer observe automatiquement la durée à la drop (RAII).
        let _timer = self.metrics.intercept_plan_duration.start_timer();

        // 1. Détermine project_id (nouveau ou réutilise)
        let project_id = match request.project_id {
            Some(id) => id,
            None => cortex_core::generate_project_id(),
        };

        // 2. Session 6 (option C) : get-or-spawn l'actor du projet
        // Race-free car chaque project_id a sa propre mailbox.
        let actor = self.get_or_spawn_actor(&project_id).await?;

        // 3. Court-circuit si le projet est déjà aborted
        if actor.is_aborted().await.unwrap_or(false) {
            return Err(CortexServerError::ValidationError(format!(
                "project {} is aborted",
                project_id
            )));
        }

        // 4. WAL prepare : log la demande
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

        // 5. Appelle l'Architect pour générer le plan
        self.metrics.inc_llm_requests();
        let plan = self
            .architect
            .generate_plan(&request.intent, &request.context)
            .await?;

        // 6. Génère résumé user-friendly
        let summary = build_summary(&plan);

        // 7. WAL commit : enregistre la réponse
        self.wal.write_commit(&entry_id).await?;

        // 8. Session 6 : enregistre le plan dans l'actor
        // L'actor owns maintenant ce state (race-free).
        let _ = actor.record_plan(plan.clone()).await;

        // 9. WAL snapshot : sauvegarde du plan comme commit historique
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
        let _timer = self.metrics.pre_mortem_duration.start_timer();
        let brain = cortex_brains::PreMortem::new(self.llm_client.clone());
        self.metrics.inc_llm_requests();
        let result = brain
            .generate_guardrails(
                &request.job_id,
                &request.job_description,
                &request.definition_of_done,
                &request.context,
                self.hmac_secret.as_deref(),
            )
            .await?;
        self.metrics
            .add_pre_mortem_guards(result.guardrails.len() as u64);

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
        let _timer = self.metrics.red_team_audit_duration.start_timer();
        // Couche 1 : HMAC integrity (non-LLM, fast).
        // On appelle directement cortex_security::verify_guardrails pour éviter
        // le type inference problem de `RedTeam::<C>::verify_hmac` (C non utilisé).
        let hmac_check: Result<(), String> = match (
            self.hmac_secret.as_deref(),
            request.guardrails_signature.as_deref(),
        ) {
            (Some(secret), Some(expected)) => {
                let payload = serde_json::json!({
                    "job_id": &request.job_id,
                    "guardrails": &request.guardrails,
                    "definition_of_done": &request.definition_of_done,
                });
                cortex_security::verify_guardrails(secret, &payload, expected)
                    .map_err(|e| format!("{}", e))
            }
            (None, Some(_)) => {
                tracing::warn!("Guardrails have HMAC signature but no server secret — skipping verify");
                Ok(())
            }
            (Some(_), None) => {
                tracing::warn!("Server has HMAC secret but guardrails are unsigned — possible downgrade");
                Ok(())
            }
            (None, None) => Ok(()),
        };
        match hmac_check {
            Ok(()) => self.metrics.inc_hmac_ok(),
            Err(e) => {
                self.metrics.inc_hmac_fail();
                return Err(CortexServerError::InternalError(format!(
                    "HMAC verification failed: {}",
                    e
                )));
            }
        }

        let brain = cortex_brains::RedTeam::new(self.llm_client.clone());
        self.metrics.inc_llm_requests();
        let result = brain
            .audit(
                &request.job_id,
                &request.definition_of_done,
                request.convergence_contract.as_deref(),
                &request.guardrails,
                &request.artifact,
            )
            .await?;

        if !result.passed {
            self.metrics.inc_red_team_blocks();
        }

        Ok(RedTeamAuditResponse {
            job_id: result.job_id,
            passed: result.passed,
            issues: result.issues,
            approved_layers: result.approved_layers,
            summary: result.summary,
        })
    }

    /// Outil `harvest_insights` : extrait patterns réutilisables d'un thème complété.
    pub async fn harvest_insights(
        &self,
        request: HarvestInsightsRequest,
    ) -> Result<HarvestInsightsResponse, CortexServerError> {
        let _timer = self.metrics.harvest_insights_duration.start_timer();
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

    // =========================================================================
    // Worker lifecycle tools (Session 4)
    // =========================================================================

    /// Outil `approve_and_execute` : user a approuvé le plan, on génère
    /// l'ordre de dispatch ordonné par phases parallélisables.
    pub async fn approve_and_execute(
        &self,
        request: ApproveAndExecuteRequest,
    ) -> Result<ApproveAndExecuteResponse, CortexServerError> {
        // Session 6 (option A) : démarre le timer pour histogramme Prometheus.
        let _timer = self.metrics.approve_and_execute_duration.start_timer();

        // 1. Récupère le plan (depuis la requête ou depuis le state cache)
        let plan = if let Some(p) = request.plan {
            p
        } else {
            // Lookup dans WAL state cache
            let state = self
                .wal
                .load_state(&request.project_id)
                .await
                .map_err(|e| CortexServerError::WalError(e.to_string()))?
                .ok_or_else(|| {
                    CortexServerError::ProjectNotFound(format!(
                        "No plan found for project {} (state cache empty)",
                        request.project_id
                    ))
                })?;
            let snapshot = &state["plan"];
            serde_json::from_value(snapshot.clone())
                .map_err(|e| CortexServerError::InternalError(format!("Invalid cached plan: {}", e)))?
        };

        // 2. Log approval dans WAL
        let entry_id = self
            .wal
            .write_prepare(
                &request.project_id,
                "approval_received",
                None,
                None,
                &serde_json::json!({
                    "approved_by": request.approved_by,
                    "themes_count": plan.themes.len(),
                }),
            )
            .await?;
        self.wal.write_commit(&entry_id).await?;
        self.wal
            .write_snapshot_commit(
                &request.project_id,
                "execution_approved",
                "approval_received",
                &serde_json::json!({"approved_by": &request.approved_by}),
                &serde_json::json!({"plan": &plan}),
            )
            .await?;

        // 3. Build dispatch order (topological sort by phase)
        let (phases, total_tokens) = build_dispatch_phases(&plan);
        let total_themes = plan.themes.len() as u32;

        // 4. Cost gating summary
        let cost_summary = cortex_brains::CostGating::summarize(&plan);

        // 5. Metrics : count dispatched jobs
        self.metrics.inc_jobs_dispatched();
        for theme in &plan.themes {
            for _ in &theme.tasks {
                self.metrics.inc_jobs_dispatched();
            }
        }

        let instructions = format!(
            "## Plan approuvé ✓\n\
             Projet : {}\n\
             Approuvé par : {}\n\n\
             **Phases d'exécution** : {} phases séquentielles\n\
             **{} thèmes total, ~{} tokens estimés**\n\n\
             ### Étapes\n\
             1. Hermes spawn Phase 1 ({} thèmes en parallèle)\n\
             2. À chaque job, vérifier criticité :\n   \
                • ≥ 4 → appel pre_mortem avant dispatch\n   \
                • ≥ 3 → red_team_audit après worker terminé\n\
             3. Hermes appelle sync_reflect après chaque job\n\
             4. Une fois Phase 1 complétée, spawn Phase 2, etc.\n\n\
             {}\n",
            request.project_id,
            request.approved_by,
            phases.len(),
            total_themes,
            total_tokens,
            phases.first().map(|p| p.len()).unwrap_or(0),
            cost_summary,
        );

        let dispatch_order = DispatchOrder {
            project_id: request.project_id.clone(),
            phases,
            total_themes,
            total_estimated_tokens: total_tokens,
            cost_gating_summary: cost_summary,
        };

        Ok(ApproveAndExecuteResponse {
            dispatch_order,
            instructions,
        })
    }

    /// Outil `sync_reflect` : Hermes appelle après worker terminé pour validation.
    /// Wrapper qui appelle red_team_audit + enregistre dans WAL.
    pub async fn sync_reflect(
        &self,
        request: SyncReflectRequest,
    ) -> Result<SyncReflectResponse, CortexServerError> {
        let _timer = self.metrics.sync_reflect_duration.start_timer();
        // 0. Session 6 (option C) : get-or-spawn l'actor et check aborted
        let actor = self.get_or_spawn_actor(&request.project_id).await?;
        if actor.is_aborted().await.unwrap_or(false) {
            return Err(CortexServerError::ValidationError(format!(
                "project {} is aborted",
                request.project_id
            )));
        }

        // 1. Run Red-Team audit
        let audit = self
            .red_team_audit(crate::server::RedTeamAuditRequest {
                job_id: request.job_id.clone(),
                definition_of_done: request.definition_of_done.clone(),
                convergence_contract: request.convergence_contract.clone(),
                guardrails: vec![],
                artifact: request.artifact.clone(),
                guardrails_signature: request.guardrails_signature.clone(),
            })
            .await?;

        // 2. Decide action
        let action = if audit.passed {
            "commit"
        } else if audit.issues.iter().any(|i| i.severity == cortex_brains::red_team::Severity::Critical) {
            "escalate"
        } else {
            "retry"
        }
        .to_string();

        // 3. Log in WAL
        let entry_id = self
            .wal
            .write_prepare(
                &request.project_id,
                "sync_reflect",
                Some(&request.job_id),
                None,
                &serde_json::json!({
                    "action": &action,
                    "passed": audit.passed,
                    "issues_count": audit.issues.len(),
                }),
            )
            .await?;
        self.wal.write_commit(&entry_id).await?;

        // 4. Metrics : track outcomes
        if action == "commit" {
            self.metrics.inc_jobs_approved();
        } else if action == "escalate" {
            self.metrics.inc_jobs_escalated();
        } else {
            self.metrics.inc_jobs_rejected(); // retry
        }

        // 5. Session 6 : enregistre le JobResult dans l'actor (race-free)
        let audit_record = AuditRecord {
            job_id: request.job_id.clone(),
            passed: audit.passed,
            action: action.clone(),
            timestamp_ms: chrono::Utc::now().timestamp_millis(),
            issues_count: audit.issues.len(),
        };
        let status = if action == "commit" {
            "committed"
        } else if action == "escalate" {
            "escalated"
        } else {
            "retried"
        };
        let _ = actor
            .record_job_result(JobResult {
                job_id: request.job_id.clone(),
                status: status.to_string(),
                last_action: action.clone(),
                last_audit: Some(audit_record),
            })
            .await;

        Ok(SyncReflectResponse {
            job_id: request.job_id,
            passed: audit.passed,
            approved: audit.passed,
            action,
            audit: audit,
        })
    }

    /// Outil `check_jobs_status` : retourne l'état courant d'un projet.
    pub async fn check_jobs_status(
        &self,
        request: CheckJobsStatusRequest,
    ) -> Result<CheckJobsStatusResponse, CortexServerError> {
        // 1. List commits (project history)
        let commits = self
            .wal
            .list_commits(&request.project_id)
            .await
            .map_err(|e| CortexServerError::WalError(e.to_string()))?;
        let last_commit_at = commits.first().map(|c| c.timestamp);
        let total_commits = commits.len() as u32;

        // 2. Load current state (plan, themes, etc.)
        let state = self
            .wal
            .load_state(&request.project_id)
            .await
            .map_err(|e| CortexServerError::WalError(e.to_string()))?;
        let handoff_summary = state
            .as_ref()
            .and_then(|s| s.get("handoff_summary").and_then(|v| v.as_str()))
            .map(String::from);

        // 3. Build theme status list
        let themes: Vec<ThemeStatus> = if let Some(state) = state {
            if let Some(plan) = state.get("plan") {
                if let Ok(plan) =
                    serde_json::from_value::<cortex_brains::FractalPlan>(plan.clone())
                {
                    plan.themes
                        .iter()
                        .map(|t| {
                            // Check if there's a sync_reflect commit for this theme's first task
                            let jobs_status =
                                infer_theme_status(&commits, &t.id, &t.tasks);
                            ThemeStatus {
                                theme_id: t.id.clone(),
                                name: t.name.clone(),
                                criticity_score: t.criticity_score,
                                tasks_count: t.tasks.len() as u32,
                                jobs_status,
                            }
                        })
                        .collect()
                } else {
                    vec![]
                }
            } else {
                vec![]
            }
        } else {
            vec![]
        };

        Ok(CheckJobsStatusResponse {
            project_id: request.project_id,
            total_commits,
            last_commit_at,
            themes,
            handoff_summary,
        })
    }

    /// Outil `rollback` : restore un état précédent d'un projet.
    pub async fn rollback(
        &self,
        request: RollbackRequest,
    ) -> Result<RollbackResponse, CortexServerError> {
        let commits = self
            .wal
            .list_commits(&request.project_id)
            .await
            .map_err(|e| CortexServerError::WalError(e.to_string()))?;
        if commits.len() < 2 && request.target_commit_id.is_none() {
            return Err(CortexServerError::ValidationError(
                "Cannot rollback: project has fewer than 2 commits".to_string(),
            ));
        }

        let current_commit_id = commits
            .first()
            .map(|c| c.commit_id.clone())
            .ok_or_else(|| CortexServerError::ProjectNotFound(request.project_id.clone()))?;

        let target = if let Some(t) = request.target_commit_id {
            commits
                .iter()
                .find(|c| c.commit_id == t)
                .ok_or_else(|| {
                    CortexServerError::ValidationError(format!("commit {} not found", t))
                })?
                .clone()
        } else {
            // Default = previous commit
            commits
                .get(1)
                .ok_or_else(|| {
                    CortexServerError::ValidationError("No previous commit".to_string())
                })?
                .clone()
        };

        // Restore state
        self.wal
            .save_state(
                &request.project_id,
                &target.snapshot,
                Some(&format!("Rolled back to {}: {}", target.commit_id, request.reason)),
            )
            .await
            .map_err(|e| CortexServerError::WalError(e.to_string()))?;

        // Log rollback
        let entry_id = self
            .wal
            .write_prepare(
                &request.project_id,
                "rollback",
                None,
                None,
                &serde_json::json!({
                    "from": &current_commit_id,
                    "to": &target.commit_id,
                    "reason": &request.reason,
                }),
            )
            .await?;
        self.wal.write_commit(&entry_id).await?;

        Ok(RollbackResponse {
            project_id: request.project_id,
            rolled_back_from: current_commit_id,
            rolled_back_to: target.commit_id,
            restored_state: target.snapshot,
        })
    }

    /// Outil `abort` : emergency stop d'un projet.
    pub async fn abort(&self, request: AbortRequest) -> Result<AbortResponse, CortexServerError> {
        let now = chrono::Utc::now().timestamp_millis();

        // Session 6 (option C) : get-or-spawn l'actor et mark aborted.
        // Toutes les opérations ultérieures sur ce projet court-circuiteront.
        let actor = self.get_or_spawn_actor(&request.project_id).await?;
        let _ = actor
            .mark_aborted(request.reason.clone(), now)
            .await;

        let entry_id = self
            .wal
            .write_prepare(
                &request.project_id,
                "abort",
                None,
                None,
                &serde_json::json!({
                    "reason": &request.reason,
                    "aborted_at": now,
                }),
            )
            .await?;
        self.wal.write_commit(&entry_id).await?;
        self.wal
            .write_snapshot_commit(
                &request.project_id,
                "aborted",
                "abort",
                &serde_json::json!({"reason": &request.reason}),
                &serde_json::json!({"status": "aborted", "aborted_at": now}),
            )
            .await?;

        Ok(AbortResponse {
            project_id: request.project_id,
            aborted: true,
            reason: request.reason,
            aborted_at: now,
        })
    }

    /// Outil `recover_project` : recovery post-crash d'un projet.
    ///
    /// Appelé typiquement au démarrage du serveur ou via un cron.
    /// Liste les entries uncommitted et applique les politiques :
    /// - actions terminales (abort, rollback, approval_received) → commit
    /// - actions in-flight (sync_reflect, task_completed) → escalate
    /// - reste → rollback
    pub async fn recover_project(
        &self,
        request: RecoverProjectRequest,
    ) -> Result<cortex_core::RecoveryReport, CortexServerError> {
        let _timer = self.metrics.recover_project_duration.start_timer();
        let report = self
            .wal
            .recover_uncommitted(&request.project_id)
            .await
            .map_err(|e| CortexServerError::WalError(e.to_string()))?;
        // Metrics : record recovery outcomes
        self.metrics
            .add_recovery_rolled_back(report.rolled_back.len() as u64);
        self.metrics
            .add_recovery_escalated(report.escalated.len() as u64);

        // Session 6 (option C) : clear le flag aborted de l'actor après recovery
        // réussi, pour permettre au projet de reprendre normalement.
        if let Ok(actor) = self
            .actor_registry
            .get_or_create(&request.project_id, &request.project_id, "")
        {
            let _ = actor.clear_aborted().await;
        }

        Ok(report)
    }

    /// Outil `get_metrics` : retourne les compteurs Prometheus en text/plain.
    pub fn get_metrics_prometheus(&self) -> String {
        self.metrics.to_prometheus()
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
// Helpers (private) — Dispatch topology + status inference
// ============================================================================

use cortex_brains::PlannedTheme;
use cortex_core::CortexCommit;

/// Topological sort des themes en phases parallélisables.
///
/// Algorithme : Kahn's algorithm simplifié.
fn build_dispatch_phases(plan: &cortex_brains::FractalPlan) -> (Vec<Vec<DispatchableTheme>>, u32) {
    let mut remaining: Vec<PlannedTheme> = plan.themes.clone();
    let mut phases: Vec<Vec<DispatchableTheme>> = Vec::new();
    let mut total_tokens: u32 = 0;

    while !remaining.is_empty() {
        // IDs de tous les themes déjà placés dans phases antérieures
        let prev_ids: Vec<String> = phases
            .iter()
            .flatten()
            .map(|d| d.theme_id.clone())
            .collect();

        // Phase courante : themes dont TOUTES les dépendances sont déjà dans phases antérieures
        let mut in_phase: Vec<PlannedTheme> = Vec::new();
        let mut still_remaining: Vec<PlannedTheme> = Vec::new();
        for t in remaining.into_iter() {
            let mut all_ok = true;
            for dep in &t.depends_on {
                if prev_ids.contains(dep) {
                    continue;
                }
                // dep n'est pas dans prev_ids → check si dans remaining
                let in_remaining = still_remaining
                    .iter()
                    .chain(in_phase.iter())
                    .any(|x| &x.id == dep);
                if in_remaining {
                    all_ok = false;
                    break;
                }
            }
            if all_ok {
                in_phase.push(t);
            } else {
                still_remaining.push(t);
            }
        }

        // 4. Always reassign `remaining` at the end of the loop body so
        //    the while-condition is well-defined even on the `continue` path.
        if in_phase.is_empty() {
            // Cycle / cassé — force le premier pour ne pas boucler
            let theme = still_remaining.remove(0);
            let dt = theme_to_dispatchable(&theme, true);
            total_tokens += dt.estimated_tokens;
            phases.push(vec![dt]);
            remaining = still_remaining;
            continue;
        }

        let mut phase_themes: Vec<DispatchableTheme> = Vec::new();
        for t in in_phase {
            let dt = theme_to_dispatchable(&t, false);
            total_tokens += dt.estimated_tokens;
            phase_themes.push(dt);
        }
        phases.push(phase_themes);
        remaining = still_remaining;
    }

    (phases, total_tokens)
}

fn theme_to_dispatchable(theme: &PlannedTheme, forced: bool) -> DispatchableTheme {
    let criticity = theme.criticity_score;
    let estimated = cortex_brains::CostGating::estimate_tokens(criticity);

    DispatchableTheme {
        theme_id: theme.id.clone(),
        theme_name: theme.name.clone(),
        criticity_score: criticity,
        estimated_tokens: estimated,
        depends_on: theme.depends_on.clone(),
        guardrails_request: criticity >= 4 || forced,
        red_team_audit_request: criticity >= 3,
        tasks: theme.tasks.clone(),
    }
}

/// Infère le status d'un thème depuis l'historique des commits.
fn infer_theme_status(
    commits: &[CortexCommit],
    theme_id: &str,
    _tasks: &[cortex_brains::PlannedTask],
) -> String {
    let mut approved = 0;
    let mut escalated = 0;
    let mut last_was_abort = false;

    for c in commits {
        if let Some(tid) = c.diff.get("theme_id").and_then(|v| v.as_str()) {
            if tid == theme_id {
                match c.mutation_type.as_str() {
                    "sync_reflect" | "task_completed" => {
                        if c.diff.get("passed").and_then(|v| v.as_bool()) == Some(true) {
                            approved += 1;
                        } else if c.diff.get("action").and_then(|v| v.as_str())
                            == Some("escalate")
                        {
                            escalated += 1;
                        }
                    }
                    "abort" => {
                        last_was_abort = true;
                    }
                    _ => {}
                }
            }
        }
    }

    if last_was_abort {
        "failed".to_string()
    } else if escalated > 0 {
        "failed".to_string()
    } else if approved > 0 {
        "completed".to_string()
    } else {
        "pending".to_string()
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use cortex_brains::MockLlmClient;
    use cortex_core::WalService as TestedWalService;
    use std::time::Duration;

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

    // ========================================================================
    // Metrics integration tests (Session 5.3)
    // ========================================================================

    #[tokio::test]
    async fn test_metrics_incremented_on_pre_mortem() {
        use std::sync::atomic::Ordering;
        let metrics = cortex_core::SharedMetrics::default();
        // Direct incrément test (sans passer par les handlers qui demandent un LLM complexe)
        metrics.inc_llm_requests();
        metrics.add_pre_mortem_guards(3);
        assert!(metrics.llm_requests_total.load(Ordering::Relaxed) >= 1);
        assert_eq!(metrics.pre_mortem_guards_emitted.load(Ordering::Relaxed), 3);
    }

    #[tokio::test]
    async fn test_metrics_prometheus_format_from_server() {
        let metrics = cortex_core::SharedMetrics::default();
        metrics.inc_jobs_dispatched();
        metrics.inc_jobs_approved();
        metrics.inc_hmac_fail();

        let out = metrics.to_prometheus();
        assert!(out.contains("cortex_jobs_dispatched_total 1"));
        assert!(out.contains("cortex_jobs_approved_total 1"));
        assert!(out.contains("cortex_hmac_verifications_total{result=\"fail\"} 1"));
        assert!(out.contains("# TYPE cortex_uptime_seconds gauge"));
    }

    #[test]
    fn test_metrics_sync_reflect_outcomes() {
        // Test direct incréments des outcomes (commit/escalate/retry)
        let metrics = cortex_core::SharedMetrics::default();
        metrics.inc_jobs_dispatched();
        metrics.inc_jobs_approved();
        metrics.inc_jobs_escalated();
        metrics.inc_jobs_rejected();

        let text = metrics.to_prometheus();
        assert!(text.contains("cortex_jobs_dispatched_total 1"));
        assert!(text.contains("cortex_jobs_approved_total 1"));
        assert!(text.contains("cortex_jobs_escalated_total 1"));
        assert!(text.contains("cortex_jobs_rejected_total 1"));
    }

    // ============================================================
    // Session 6 (option C) : tests actor isolation
    // ============================================================

    /// Helper : crée un CortexServer avec un MockLlmClient qui retourne un plan valide.
    async fn make_test_server() -> CortexServer<cortex_brains::MockLlmClient> {
        let wal = cortex_core::WalService::connect("sqlite::memory:")
            .await
            .expect("in-memory WAL");
        let mock = cortex_brains::MockLlmClient::with_response(VALID_PLAN_JSON.to_string());
        let architect = Architect::new(mock.clone());
        CortexServer::new(wal, architect, mock)
    }

    #[tokio::test]
    async fn test_actor_registry_count_after_intercept_plan() {
        // Chaque intercept_plan spawn un actor pour son project_id.
        let server = make_test_server().await;
        assert_eq!(server.actor_registry.count(), 0);

        let r1 = server
            .intercept_plan(InterceptPlanRequest {
                project_id: Some("proj-A".into()),
                intent: "Test A".into(),
                context: "ctx".into(),
            })
            .await
            .expect("plan A");

        let r2 = server
            .intercept_plan(InterceptPlanRequest {
                project_id: Some("proj-B".into()),
                intent: "Test B".into(),
                context: "ctx".into(),
            })
            .await
            .expect("plan B");

        assert_eq!(server.actor_registry.count(), 2);
        assert_eq!(r1.project_id, "proj-A");
        assert_eq!(r2.project_id, "proj-B");
    }

    #[tokio::test]
    async fn test_actor_state_recorded_after_intercept_plan() {
        // Après intercept_plan, l'actor a le plan en state.
        let server = make_test_server().await;
        let resp = server
            .intercept_plan(InterceptPlanRequest {
                project_id: Some("proj-state".into()),
                intent: "Build a CLI".into(),
                context: "Rust".into(),
            })
            .await
            .expect("plan");

        let actor = server
            .actor_registry
            .get_or_create(&resp.project_id, &resp.project_id, "")
            .expect("actor");
        let snap = actor.get_state().await.expect("snap");
        assert!(snap.has_plan, "plan should be recorded in actor state");
        assert!(
            snap.plan_themes_count > 0,
            "plan should have themes (got {})",
            snap.plan_themes_count
        );
        assert!(!snap.aborted, "fresh project should not be aborted");
    }

    #[tokio::test]
    async fn test_actor_isolation_between_projects() {
        // 2 projets distincts = 2 actors distincts avec state vraiment isolé.
        let server = make_test_server().await;
        server
            .intercept_plan(InterceptPlanRequest {
                project_id: Some("proj-X".into()),
                intent: "Refactor auth".into(),
                context: "FastAPI".into(),
            })
            .await
            .expect("X");
        server
            .intercept_plan(InterceptPlanRequest {
                project_id: Some("proj-Y".into()),
                intent: "Export PDF".into(),
                context: "reportlab".into(),
            })
            .await
            .expect("Y");

        let actor_x = server
            .actor_registry
            .get_or_create("proj-X", "proj-X", "")
            .expect("actor X");
        let actor_y = server
            .actor_registry
            .get_or_create("proj-Y", "proj-Y", "")
            .expect("actor Y");

        let snap_x = actor_x.get_state().await.expect("snap X");
        let snap_y = actor_y.get_state().await.expect("snap Y");

        // Les 2 actors ont chacun leur plan enregistré
        assert!(snap_x.has_plan);
        assert!(snap_y.has_plan);
        assert_eq!(snap_x.project_id, "proj-X");
        assert_eq!(snap_y.project_id, "proj-Y");
    }

    #[tokio::test]
    async fn test_abort_blocks_subsequent_intercept_plan() {
        // Après abort, intercept_plan sur le même projet doit être refusé.
        let server = make_test_server().await;
        let r1 = server
            .intercept_plan(InterceptPlanRequest {
                project_id: Some("proj-blocked".into()),
                intent: "First".into(),
                context: "".into(),
            })
            .await
            .expect("first plan");

        // Abort
        server
            .abort(AbortRequest {
                project_id: r1.project_id.clone(),
                reason: "user cancelled".into(),
            })
            .await
            .expect("abort");

        // Vérif que l'actor est aborted
        let actor = server
            .actor_registry
            .get_or_create(&r1.project_id, &r1.project_id, "")
            .expect("actor");
        assert!(actor.is_aborted().await.expect("is_aborted"));

        // Tentative d'intercept_plan → doit échouer
        let r2 = server
            .intercept_plan(InterceptPlanRequest {
                project_id: Some(r1.project_id.clone()),
                intent: "Second".into(),
                context: "".into(),
            })
            .await;
        assert!(
            r2.is_err(),
            "intercept_plan should fail on aborted project, got {:?}",
            r2
        );
    }

    // ============================================================
    // Session 6 (option A) : tests histogram instrumentation
    // ============================================================

    #[tokio::test]
    async fn test_histogram_incremented_on_intercept_plan() {
        // Vérifie qu'un appel intercept_plan incrémente bien l'histogramme.
        let server = make_test_server().await;

        // Avant : count = 0
        let snap_before = server.metrics.intercept_plan_duration.snapshot();
        assert_eq!(snap_before.count, 0, "histogram should start at 0");

        // 1 appel
        server
            .intercept_plan(InterceptPlanRequest {
                project_id: Some("histogram-test".into()),
                intent: "X".into(),
                context: "Y".into(),
            })
            .await
            .expect("plan");

        // Après : count = 1
        let snap_after = server.metrics.intercept_plan_duration.snapshot();
        assert_eq!(snap_after.count, 1, "histogram should have 1 observation");
        assert!(snap_after.sum_micros > 0, "duration should be > 0");
    }

    #[tokio::test]
    async fn test_histogram_prometheus_format_includes_all_8() {
        // Vérifie que le format Prometheus contient bien les 8 histogrammes.
        let server = make_test_server().await;
        // On observe chaque histogramme une fois
        server.metrics.intercept_plan_duration.observe(Duration::from_millis(10));
        server.metrics.pre_mortem_duration.observe(Duration::from_millis(20));
        server.metrics.red_team_audit_duration.observe(Duration::from_millis(30));
        server.metrics.sync_reflect_duration.observe(Duration::from_millis(40));
        server.metrics.approve_and_execute_duration.observe(Duration::from_millis(50));
        server.metrics.recover_project_duration.observe(Duration::from_millis(60));
        server.metrics.harvest_insights_duration.observe(Duration::from_millis(70));
        server.metrics.llm_request_duration.observe(Duration::from_millis(80));

        let out = server.get_metrics_prometheus();
        // Vérif que les 8 histogrammes sont présents
        for name in [
            "cortex_intercept_plan_duration_seconds",
            "cortex_pre_mortem_duration_seconds",
            "cortex_red_team_audit_duration_seconds",
            "cortex_sync_reflect_duration_seconds",
            "cortex_approve_and_execute_duration_seconds",
            "cortex_recover_project_duration_seconds",
            "cortex_harvest_insights_duration_seconds",
            "cortex_llm_request_duration_seconds",
        ] {
            assert!(
                out.contains(&format!("# TYPE {} histogram", name)),
                "missing histogram {} in output",
                name
            );
            assert!(
                out.contains(&format!("{}_count 1", name)),
                "missing count for {} in output",
                name
            );
        }
    }
}
