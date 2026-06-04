//! ProjectState : owned by exactly one ProjectActor (no Arc, no Mutex).
//!
//! Encapsulates all per-project mutable state that previously lived in
//! CortexServer. Access is serialized through the actor's mailbox — no
//! locks, no race conditions.

use std::collections::HashMap;

use cortex_brains::FractalPlan;
use cortex_core::Scratchpad;
use serde::{Deserialize, Serialize};

/// Audit record of a single Red-Team check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditRecord {
    pub job_id: String,
    pub passed: bool,
    pub action: String, // "commit" | "retry" | "escalate"
    pub timestamp_ms: i64,
    pub issues_count: usize,
}

/// Result of a single sync_reflect call (cached for status queries).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobResult {
    pub job_id: String,
    pub status: String, // "committed" | "retried" | "escalated" | "pending"
    pub last_action: String,
    pub last_audit: Option<AuditRecord>,
}

/// Per-project mutable state owned by ONE ProjectActor.
///
/// Invariants :
/// - `plan` is set after first `InterceptPlan` message
/// - `job_results` is keyed by job_id, populated by `SyncReflect`
/// - `aborted` flag short-circuits all subsequent operations
#[derive(Debug, Clone)]
pub struct ProjectState {
    pub project_id: String,
    pub scratchpad: Scratchpad,
    pub plan: Option<FractalPlan>,
    pub job_results: HashMap<String, JobResult>,
    pub audit_history: Vec<AuditRecord>,
    pub aborted: bool,
    pub aborted_at_ms: Option<i64>,
    pub aborted_reason: Option<String>,
}

impl ProjectState {
    /// Crée un ProjectState vide. Le scratchpad est créé avec un nom/objectif
    /// vide car on les découvrira dans InterceptPlan.
    pub fn new(project_id: String) -> Self {
        // Le scratchpad peut échouer si project_id est vide (ce qui n'est pas
        // notre cas ici). Si ça échoue, on log et on met un stub.
        let scratchpad =
            Scratchpad::new(project_id.clone(), String::new(), String::new()).unwrap_or_else(
                |_| Scratchpad {
                    project_id: project_id.clone(),
                    ..Default::default()
                },
            );

        Self {
            project_id,
            scratchpad,
            plan: None,
            job_results: HashMap::new(),
            audit_history: Vec::new(),
            aborted: false,
            aborted_at_ms: None,
            aborted_reason: None,
        }
    }

    /// Marque le projet comme aborted. Toute opération ultérieure doit
    /// court-circuiter.
    pub fn mark_aborted(&mut self, reason: String, timestamp_ms: i64) {
        self.aborted = true;
        self.aborted_reason = Some(reason);
        self.aborted_at_ms = Some(timestamp_ms);
    }

    /// Enregistre un job result.
    pub fn record_job_result(&mut self, result: JobResult) {
        self.job_results.insert(result.job_id.clone(), result);
    }

    /// Enregistre un audit.
    pub fn record_audit(&mut self, audit: AuditRecord) {
        self.audit_history.push(audit);
    }

    /// Liste les job_ids connus.
    pub fn known_jobs(&self) -> Vec<String> {
        self.job_results.keys().cloned().collect()
    }

    /// Compte les jobs par status.
    pub fn jobs_by_status(&self) -> HashMap<String, usize> {
        let mut counts = HashMap::new();
        for r in self.job_results.values() {
            *counts.entry(r.status.clone()).or_insert(0) += 1;
        }
        counts
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_state_has_empty_collections() {
        let state = ProjectState::new("p1".to_string());
        assert_eq!(state.project_id, "p1");
        assert!(state.plan.is_none());
        assert!(state.job_results.is_empty());
        assert!(state.audit_history.is_empty());
        assert!(!state.aborted);
    }

    #[test]
    fn test_mark_aborted_sets_flags() {
        let mut state = ProjectState::new("p1".to_string());
        state.mark_aborted("user cancelled".into(), 12345);
        assert!(state.aborted);
        assert_eq!(state.aborted_reason.as_deref(), Some("user cancelled"));
        assert_eq!(state.aborted_at_ms, Some(12345));
    }

    #[test]
    fn test_record_job_result_inserts() {
        let mut state = ProjectState::new("p1".to_string());
        state.record_job_result(JobResult {
            job_id: "J-1".into(),
            status: "committed".into(),
            last_action: "commit".into(),
            last_audit: None,
        });
        assert_eq!(state.known_jobs(), vec!["J-1".to_string()]);
        let counts = state.jobs_by_status();
        assert_eq!(counts.get("committed"), Some(&1));
    }

    #[test]
    fn test_record_audit_appends() {
        let mut state = ProjectState::new("p1".to_string());
        state.record_audit(AuditRecord {
            job_id: "J-1".into(),
            passed: true,
            action: "commit".into(),
            timestamp_ms: 1000,
            issues_count: 0,
        });
        state.record_audit(AuditRecord {
            job_id: "J-2".into(),
            passed: false,
            action: "escalate".into(),
            timestamp_ms: 2000,
            issues_count: 1,
        });
        assert_eq!(state.audit_history.len(), 2);
        assert!(state.audit_history[0].passed);
        assert!(!state.audit_history[1].passed);
    }
}
