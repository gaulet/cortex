//! Scratchpad: per-project volatile state managed by Cortex.
//!
//! A `Scratchpad` holds the current state of a project: its objective, themes,
//! jobs, milestones, parking lot, ignored noise, and a WAL-style log of all
//! mutations. The WAL log lets Cortex rebuild the full state after a crash.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{CortexError, Result};

/// Per-project state managed by Cortex.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scratchpad {
    pub scratchpad_id: String,
    pub project_id: String,
    pub project_name: String,
    pub created_at: DateTime<Utc>,
    pub last_updated: DateTime<Utc>,
    pub current_commit: String,
    pub status: ScratchpadStatus,

    pub objective: String,
    pub handoff_summary: Option<String>,

    pub themes: HashMap<String, Theme>,
    pub jobs: HashMap<String, Job>,
    pub milestones: Vec<Milestone>,
    pub parking_lot: Vec<ParkingIdea>,
    pub ignored_noise: Vec<IgnoredConstraint>,
    pub wal_log: Vec<WalLogEntry>,
}

impl Scratchpad {
    /// Create a new scratchpad for a fresh project.
    pub fn new(project_id: String, project_name: String, objective: String) -> Result<Self> {
        if project_id.is_empty() {
            return Err(CortexError::ValidationError("project_id is empty".into()));
        }
        let commit_id = format!("commit_{}", Uuid::now_v7());
        let now = Utc::now();
        Ok(Self {
            scratchpad_id: format!("scratchpad_{}", Uuid::now_v7()),
            project_id,
            project_name,
            created_at: now,
            last_updated: now,
            current_commit: commit_id.clone(),
            status: ScratchpadStatus::Active,

            objective,
            handoff_summary: None,

            themes: HashMap::new(),
            jobs: HashMap::new(),
            milestones: Vec::new(),
            parking_lot: Vec::new(),
            ignored_noise: Vec::new(),
            wal_log: vec![WalLogEntry {
                ts: now,
                commit: commit_id,
                mutation: "initial".to_string(),
                reason: "Scratchpad created".to_string(),
            }],
        })
    }

    /// Append a theme to the scratchpad.
    pub fn add_theme(&mut self, theme: Theme) {
        self.themes.insert(theme.id.clone(), theme);
        self.last_updated = Utc::now();
    }

    /// Append a job to the scratchpad.
    pub fn add_job(&mut self, job: Job) {
        self.jobs.insert(job.job_id.clone(), job);
        self.last_updated = Utc::now();
    }

    /// Log a mutation in the WAL log (for audit trail + crash recovery).
    pub fn log_mutation(&mut self, mutation: String, reason: String) {
        let commit_id = format!("commit_{}", Uuid::now_v7());
        self.wal_log.push(WalLogEntry {
            ts: Utc::now(),
            commit: commit_id.clone(),
            mutation,
            reason,
        });
        self.current_commit = commit_id;
        self.last_updated = Utc::now();
    }
}

impl Default for Scratchpad {
    fn default() -> Self {
        // Note: this is mostly a stub — real usage should call `Scratchpad::new`.
        // The `default_for` helper is not provided because each scratchpad
        // should have a real project_id at construction time.
        Self {
            scratchpad_id: format!("scratchpad_{}", Uuid::now_v7()),
            project_id: String::new(),
            project_name: String::new(),
            created_at: Utc::now(),
            last_updated: Utc::now(),
            current_commit: format!("commit_{}", Uuid::now_v7()),
            status: ScratchpadStatus::Active,
            objective: String::new(),
            handoff_summary: None,
            themes: HashMap::new(),
            jobs: HashMap::new(),
            milestones: Vec::new(),
            parking_lot: Vec::new(),
            ignored_noise: Vec::new(),
            wal_log: Vec::new(),
        }
    }
}

/// Lifecycle status of a scratchpad.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScratchpadStatus {
    Active,
    Hibernated,
    Completed,
    Failed,
}

/// A theme is a logical sub-project (e.g. "Audit", "Research JWT", "Impl migration").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Theme {
    pub id: String,
    pub name: String,
    pub status: ThemeStatus,
    pub is_parallel_branch: bool,
    pub convergence_contract: Option<String>,
    pub depends_on: Vec<String>,
    pub criticity_score: u8,
    pub jobs: Vec<String>,
}

/// Lifecycle status of a theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeStatus {
    Planned,
    Approved,
    Executing,
    Completed,
    Failed,
    Blocked,
}

/// A job is a unit of work executed by a Hermes worker.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub job_id: String,
    pub theme_id: String,
    pub project_id: String,
    pub task_name: String,
    pub status: JobStatus,
    pub retry_count: u8,
    pub max_retries: u8,
    pub guardrails_hash: String,
    pub guardrails: Vec<String>,
    pub definition_of_done: String,
    pub pre_mortem_constraints: Vec<String>,
    pub artifact: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Lifecycle status of a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobStatus {
    Pending,
    Running,
    Validated,
    Failed,
    Escalated,
    Retrying,
}

/// Milestone in a project.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Milestone {
    pub milestone_id: String,
    pub name: String,
    pub summary: String,
    pub artefact_link: Option<String>,
}

/// Parking lot: idea deferred to later.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParkingIdea {
    pub idea: String,
    pub priority: String,
}

/// Ignored constraint (filtered by Cortex during interception).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IgnoredConstraint {
    pub constraint: String,
    pub reason: String,
}

/// WAL log entry (mutation to the scratchpad).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalLogEntry {
    pub ts: DateTime<Utc>,
    pub commit: String,
    pub mutation: String,
    pub reason: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_scratchpad() {
        let pad = Scratchpad::new(
            "proj_1".to_string(),
            "Auth refactor".to_string(),
            "Refactor auth module".to_string(),
        )
        .expect("should create scratchpad");
        assert_eq!(pad.project_id, "proj_1");
        assert_eq!(pad.status, ScratchpadStatus::Active);
        assert_eq!(pad.wal_log.len(), 1);
        assert_eq!(pad.wal_log[0].mutation, "initial");
    }

    #[test]
    fn test_new_scratchpad_empty_id_fails() {
        let result = Scratchpad::new(String::new(), "name".into(), "obj".into());
        assert!(result.is_err());
    }

    #[test]
    fn test_add_theme() {
        let mut pad = Scratchpad::new("p1".into(), "n".into(), "o".into()).unwrap();
        let theme = Theme {
            id: "TH-1".into(),
            name: "Audit".into(),
            status: ThemeStatus::Planned,
            is_parallel_branch: true,
            convergence_contract: None,
            depends_on: vec![],
            criticity_score: 3,
            jobs: vec![],
        };
        pad.add_theme(theme);
        assert_eq!(pad.themes.len(), 1);
        assert!(pad.themes.contains_key("TH-1"));
    }

    #[test]
    fn test_log_mutation() {
        let mut pad = Scratchpad::new("p1".into(), "n".into(), "o".into()).unwrap();
        let before_commits = pad.wal_log.len();
        pad.log_mutation("theme_added".into(), "TH-1 added".into());
        assert_eq!(pad.wal_log.len(), before_commits + 1);
        assert!(pad.wal_log.last().unwrap().commit.starts_with("commit_"));
    }

    #[test]
    fn test_scratchpad_serialization_roundtrip() {
        let pad = Scratchpad::new("p1".into(), "n".into(), "o".into()).unwrap();
        let json = serde_json::to_string(&pad).expect("serialize");
        let pad2: Scratchpad = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(pad.project_id, pad2.project_id);
        assert_eq!(pad.scratchpad_id, pad2.scratchpad_id);
    }
}
