//! Tests d'intégration end-to-end pour Cortex MCP.
//!
//! Simule le flux complet : User → Hermes (routing) → Cortex.intercept_plan → Plan

use cortex_brains::{Architect, MockLlmClient};
use cortex_core::WalService;
use cortex_mcp_server::server::{CortexServer, InterceptPlanRequest};

const MOCK_LLM_RESPONSE: &str = r#"{
    "themes": [
        {
            "id": "TH-1",
            "name": "Audit sécurité",
            "is_parallel_branch": true,
            "convergence_contract": "Rapport JSON des vulnérabilités",
            "depends_on": [],
            "criticity_score": 4,
            "resources_used": ["file:src/auth/"],
            "concurrency_group": "group_A",
            "tasks": [
                {"id": "T-1.1", "name": "Scan injection SQL", "definition_of_done": "Aucune injection", "depends_on": []},
                {"id": "T-1.2", "name": "Audit password hashing", "definition_of_done": "bcrypt utilisé", "depends_on": []}
            ]
        },
        {
            "id": "TH-2",
            "name": "Research JWT libraryes",
            "is_parallel_branch": true,
            "convergence_contract": "Comparatif JSON des libs",
            "depends_on": [],
            "criticity_score": 2,
            "resources_used": ["web"],
            "concurrency_group": "group_A",
            "tasks": [
                {"id": "T-2.1", "name": "Compare PyJWT vs python-jose", "definition_of_done": "Tableau 5+ critères", "depends_on": []}
            ]
        },
        {
            "id": "TH-3",
            "name": "Implémentation migration JWT",
            "is_parallel_branch": false,
            "convergence_contract": null,
            "depends_on": ["TH-1", "TH-2"],
            "criticity_score": 5,
            "resources_used": ["file:src/auth/", "db:users"],
            "concurrency_group": "group_B",
            "tasks": [
                {"id": "T-3.1", "name": "Migration JWT refresh tokens", "definition_of_done": "Endpoint /refresh fonctionnel", "depends_on": []},
                {"id": "T-3.2", "name": "Tests integration JWT", "definition_of_done": "Coverage > 90%", "depends_on": []}
            ]
        }
    ],
    "concurrency_groups": [
        {"name": "group_A", "themes": ["TH-1", "TH-2"], "execution_mode": "parallel", "reason": "Pas de ressources partagées"},
        {"name": "group_B", "themes": ["TH-3"], "execution_mode": "sequential_after_group_A", "reason": "Dépend de TH-1 et TH-2"}
    ],
    "parking_lot": [
        {"idea": "Ajouter monitoring Prometheus", "priority": "low"},
        {"idea": "OAuth2 flow PKCE", "priority": "medium"}
    ],
    "ignored_noise": [
        {"constraint": "Utiliser PHP 4", "reason": "Incompatible Python stack"}
    ],
    "impact_warnings": [
        {"target_project": "proj_mobile", "impact_type": "shared_dependency", "description": "proj_mobile utilise aussi db:users"}
    ]
}"#;

#[tokio::test]
async fn test_full_flow_user_to_cortex() {
    // === Setup : WAL + Mock LLM + Server ===
    let wal = WalService::connect("sqlite::memory:")
        .await
        .expect("in-memory WAL");
    let mock = MockLlmClient::with_response(MOCK_LLM_RESPONSE.to_string());
    let server = CortexServer::new(wal, Architect::new(mock));

    // === Étape 1 : User exprime intention ===
    let user_intent = "Refactorise le module auth pour passer à JWT modernes";
    let complexity = 8;

    // === Étape 2 : Hermes route vers Cortex ===
    let should_route = server.should_route(user_intent, complexity);
    assert!(
        should_route,
        "Intent avec 'refactor' + complexity 8 devrait router vers Cortex"
    );

    // === Étape 3 : Hermes appelle Cortex.intercept_plan ===
    let request = InterceptPlanRequest {
        intent: user_intent.to_string(),
        context: "Projet FastAPI + PostgreSQL. 45 endpoints actifs. Base code en Python 3.11.".to_string(),
        project_id: None,
    };

    let response = server
        .intercept_plan(request)
        .await
        .expect("intercept_plan should succeed");

    // === Étape 4 : Validation du plan généré ===

    // Check project_id créé
    assert!(
        response.project_id.starts_with("project-"),
        "Project ID should start with 'project-'"
    );

    // Check approval requis
    assert!(
        response.requires_user_approval,
        "Plan requires user approval before execution"
    );

    // Check plan structure
    let plan = &response.plan;
    assert_eq!(plan.themes.len(), 3, "Should have 3 themes");

    // Check TH-1 (critique 4, parallel)
    let th1 = &plan.themes[0];
    assert_eq!(th1.id, "TH-1");
    assert_eq!(th1.criticity_score, 4);
    assert!(th1.is_parallel_branch);
    assert!(th1.depends_on.is_empty());
    assert_eq!(th1.tasks.len(), 2);

    // Check TH-3 (critique 5, sequential, depends on TH-1 + TH-2)
    let th3 = &plan.themes[2];
    assert_eq!(th3.id, "TH-3");
    assert_eq!(th3.criticity_score, 5);
    assert!(!th3.is_parallel_branch);
    assert_eq!(th3.depends_on.len(), 2);
    assert!(th3.depends_on.contains(&"TH-1".to_string()));
    assert!(th3.depends_on.contains(&"TH-2".to_string()));
    assert_eq!(th3.tasks.len(), 2);

    // Check concurrency groups
    assert_eq!(plan.concurrency_groups.len(), 2);
    assert_eq!(plan.concurrency_groups[0].themes.len(), 2); // TH-1 + TH-2 parallel

    // Check parking lot
    assert_eq!(plan.parking_lot.len(), 2);

    // Check ignored noise
    assert_eq!(plan.ignored_noise.len(), 1);
    assert!(plan.ignored_noise[0].constraint.contains("PHP"));

    // Check impact warnings
    assert_eq!(plan.impact_warnings.len(), 1);
    assert!(plan.impact_warnings[0].target_project.contains("mobile"));

    // === Étape 5 : Validation du résumé user-friendly ===
    let summary = &response.summary_for_user;
    assert!(
        summary.contains("3 thèmes"),
        "Summary should mention 3 themes, got: {}",
        summary
    );
    assert!(
        summary.contains("criticité max 5"),
        "Summary should mention max criticity"
    );
    assert!(
        summary.contains("Pre-Mortem"),
        "Summary should mention Pre-Mortem activation (criticity ≥ 4)"
    );

    // === Étape 6 : Validation WAL ===
    let commits = server
        .wal()
        .list_commits(&response.project_id)
        .await
        .expect("should list commits");
    assert!(!commits.is_empty(), "WAL should have at least one commit");
    assert_eq!(commits[0].mutation_type, "plan_generated");

    // === Étape 7 : Check uncommitted WAL entries ===
    // (Toutes les entries devraient être committées après intercept_plan)
    let uncommitted = server
        .wal()
        .list_uncommitted(&response.project_id)
        .await
        .expect("should list uncommitted");
    assert_eq!(
        uncommitted.len(),
        0,
        "All WAL entries should be committed after successful plan"
    );
}

#[tokio::test]
async fn test_user_intent_routing_threshold() {
    let wal = WalService::connect("sqlite::memory:").await.unwrap();
    let mock = MockLlmClient::with_response("".to_string());
    let server = CortexServer::new(wal, Architect::new(mock));

    // Test seuils de routing

    // Intent avec "refactor" + complexity < threshold → NO route
    assert!(!server.should_route("Refactorise le module", 4));

    // Intent avec "refactor" + complexity = threshold → route
    assert!(server.should_route("Refactorise le module", 5));

    // Intent avec "simple" → NO route (exclu)
    assert!(!server.should_route("Refactorise simple le module", 8));

    // Intent sans mot-clé déclencheur → NO route
    assert!(!server.should_route("Fix ce bug", 9));
    assert!(!server.should_route("Ajoute une feature", 7));
}

#[tokio::test]
async fn test_full_flow_with_existing_project() {
    let wal = WalService::connect("sqlite::memory:").await.unwrap();
    let mock = MockLlmClient::with_response(MOCK_LLM_RESPONSE.to_string());
    let server = CortexServer::new(wal, Architect::new(mock));

    // === User fournit project_id existant ===
    let request = InterceptPlanRequest {
        intent: "Refactorise le module auth".to_string(),
        context: "".to_string(),
        project_id: Some("project-existing-123".to_string()),
    };

    let response = server.intercept_plan(request).await.unwrap();

    // Project_id should be kept
    assert_eq!(response.project_id, "project-existing-123");

    // Commit should be on the existing project
    let commits = server
        .wal()
        .list_commits("project-existing-123")
        .await
        .unwrap();
    assert!(!commits.is_empty());
}

#[tokio::test]
async fn test_full_flow_llm_failure() {
    let wal = WalService::connect("sqlite::memory:").await.unwrap();
    let mock = MockLlmClient::with_response("LLM unavailable".to_string()); // Invalid JSON
    let server = CortexServer::new(wal, Architect::new(mock));

    let request = InterceptPlanRequest {
        intent: "Refactorise le module".to_string(),
        context: "".to_string(),
        project_id: Some("project-fail-test".to_string()),
    };

    let err = server
        .intercept_plan(request)
        .await
        .expect_err("should fail on invalid LLM response");

    match err {
        cortex_mcp_server::server::CortexServerError::InterceptPlanFailed(msg) => {
            assert!(
                msg.contains("parse") || msg.contains("JSON") || msg.contains("Response"),
                "Error should mention parsing issue: {}",
                msg
            );
        }
        other => panic!("Wrong error type: {:?}", other),
    }

    // WAL should have the prepare entry but NOT committed (rolled back)
    let uncommitted = server
        .wal()
        .list_uncommitted("project-fail-test")
        .await
        .unwrap();
    // Note: current impl keeps the prepare entry even on failure
    // (future PR could roll back on failure)
    assert!(
        uncommitted.len() <= 1,
        "Should have at most 1 uncommitted entry"
    );
}
