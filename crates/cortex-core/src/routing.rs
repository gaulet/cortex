//! Routing rules pour handshake entre Hermes et Cortex.
//!
//! # Rôle
//!
//! Le fichier `routing_rules.json` (ou la struct `RoutingRules`) définit
//! comment Hermes doit router les user intents vers Cortex :
//! - Quels patterns doivent déclencher Cortex (patterns + complexity ≥ threshold)
//! - Quels toolsets Hermes doit utiliser
//! - Règles de délégation
//!
//! # Contrat
//!
//! Hermes charge ce fichier au boot et l'utilise pour toute la session
//! (pas de hot-reload). Changements possibles uniquement au redémarrage.

use serde::{Deserialize, Serialize};

/// Règles de routage Hermes → Cortex.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingRules {
    pub version: String,
    pub enabled: bool,
    pub cortex_required: bool,
    pub intent_patterns: Vec<String>,
    pub min_complexity_threshold: u8,
    pub excluded_patterns: Vec<String>,
    pub recommended_toolsets: Vec<String>,
    pub delegation_rules: DelegationRules,
}

/// Règles spécifiques à la délégation vers Cortex.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegationRules {
    pub max_iterations: u8,
    pub default_model: String,
    pub require_artifacts_on_disk: bool,
    pub require_marker_file: bool,
    pub parallel_batch_size: u8,
}

impl RoutingRules {
    /// Charge les règles par défaut (hardcoded pour le moment).
    pub fn default_rules() -> Self {
        Self {
            version: "1.0".to_string(),
            enabled: true,
            cortex_required: true,
            // Patterns : liste de mots-clés à chercher (au moins 1 match)
            intent_patterns: vec![
                "refactor".to_string(),
                "refactorise".to_string(),
                "analyse complete".to_string(),
                "full analysis".to_string(),
                "architecture".to_string(),
                "architecte".to_string(),
                "migration".to_string(),
                "migrer".to_string(),
                "optimise performance".to_string(),
                "plan complexe".to_string(),
                "complex plan".to_string(),
                "rewrite".to_string(),
                "redesign".to_string(),
                "re-design".to_string(),
                "restructuration".to_string(),
            ],
            min_complexity_threshold: 5,
            excluded_patterns: vec![
                "simple".to_string(),
                "quick fix".to_string(),
                "one-liner".to_string(),
                "typo".to_string(),
                "juste".to_string(), // "juste corriger ça"
            ],
            recommended_toolsets: vec![
                "terminal".to_string(),
                "file".to_string(),
                "web".to_string(),
            ],
            delegation_rules: DelegationRules {
                max_iterations: 20,
                default_model: "minimax/minimax-m3".to_string(),
                require_artifacts_on_disk: true,
                require_marker_file: true,
                parallel_batch_size: 3,
            },
        }
    }

    /// Détermine si un intent devrait être routé vers Cortex.
    pub fn should_route_to_cortex(&self, intent: &str, complexity: u8) -> bool {
        if !self.enabled || !self.cortex_required {
            return false;
        }

        let intent_lower = intent.to_lowercase();

        // Vérifie complexity threshold
        if complexity < self.min_complexity_threshold {
            return false;
        }

        // Vérifie patterns exclus (fast path) - contient au moins un exclus
        for exclusion in &self.excluded_patterns {
            if intent_lower.contains(&exclusion.to_lowercase()) {
                return false;
            }
        }

        // Cherche un pattern qui match (contient au moins un mot-clé)
        for pattern in &self.intent_patterns {
            if intent_lower.contains(&pattern.to_lowercase()) {
                return true;
            }
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_rules_structure() {
        let rules = RoutingRules::default_rules();
        assert_eq!(rules.version, "1.0");
        assert!(rules.enabled);
        assert!(rules.cortex_required);
        assert!(!rules.intent_patterns.is_empty());
        assert_eq!(rules.min_complexity_threshold, 5);
    }

    #[test]
    fn test_should_route_refactor_intent() {
        let rules = RoutingRules::default_rules();
        assert!(rules.should_route_to_cortex("Refactorise le module auth", 7));
        assert!(rules.should_route_to_cortex("refactor the database layer", 8));
    }

    #[test]
    fn test_should_route_architecture_intent() {
        let rules = RoutingRules::default_rules();
        assert!(rules.should_route_to_cortex("Architecture complète du système", 6));
    }

    #[test]
    fn test_should_route_migration_intent() {
        let rules = RoutingRules::default_rules();
        assert!(rules.should_route_to_cortex("Migration de la base PostgreSQL", 7));
    }

    #[test]
    fn test_should_not_route_simple_intent() {
        let rules = RoutingRules::default_rules();
        // Pattern "simple" doit exclure
        assert!(!rules.should_route_to_cortex("Refactorise simple le module", 7));
    }

    #[test]
    fn test_should_not_route_low_complexity() {
        let rules = RoutingRules::default_rules();
        // Complexity 4 < threshold 5 → pas route
        assert!(!rules.should_route_to_cortex("Refactorise le module", 4));
    }

    #[test]
    fn test_should_not_route_disabled() {
        let mut rules = RoutingRules::default_rules();
        rules.enabled = false;
        assert!(!rules.should_route_to_cortex("Refactorise le module", 7));
    }

    #[test]
    fn test_should_not_route_non_matching_intent() {
        let rules = RoutingRules::default_rules();
        // "Fix bug" ne match aucun pattern
        assert!(!rules.should_route_to_cortex("Fix this bug", 8));
    }

    #[test]
    fn test_default_rules_serialization_roundtrip() {
        let rules = RoutingRules::default_rules();
        let json = serde_json::to_string_pretty(&rules).unwrap();
        let parsed: RoutingRules = serde_json::from_str(&json).unwrap();
        assert_eq!(rules.version, parsed.version);
        assert_eq!(rules.enabled, parsed.enabled);
        assert_eq!(rules.intent_patterns.len(), parsed.intent_patterns.len());
    }
}
