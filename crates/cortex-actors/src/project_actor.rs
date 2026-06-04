//! Project actor : one actor per project, owns its ProjectState exclusively.
//!
//! Communication is done via typed messages (mpsc) + oneshot reply channels,
//! avoiding Arc<Mutex<T>> contention entirely.
//!
//! Design (Session 6) :
//! - The actor owns state only (no WAL, no LLM, no metrics injected)
//! - The cortex-mcp-server does the actual work and asks the actor to
//!   record results via messages
//! - This keeps the actor simple and dependency-free
//! - Race conditions are impossible because all state mutations go through
//!   the actor's mailbox (serialized)

use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use cortex_brains::FractalPlan;
use cortex_core::Scratchpad;

use crate::project_state::{AuditRecord, JobResult, ProjectState};

/// Snapshot sérialisable du state d'un projet (pour export/debug).
/// On n'envoie pas le `ProjectState` complet car il contient des types
/// non-Clone (Scratchpad n'est pas Clone).
#[derive(Debug, Clone)]
pub struct ProjectStateSnapshot {
    pub project_id: String,
    pub has_plan: bool,
    pub plan_themes_count: usize,
    pub plan_total_tokens: u32,
    pub job_results: std::collections::HashMap<String, JobResult>,
    pub audit_history: Vec<AuditRecord>,
    pub aborted: bool,
    pub aborted_at_ms: Option<i64>,
    pub aborted_reason: Option<String>,
}

/// Inbound messages a ProjectActor can handle.
#[derive(Debug)]
pub enum ProjectMessage {
    /// Retourne un clone du scratchpad (compat legacy).
    GetScratchpad {
        reply: oneshot::Sender<Result<cortex_core::Scratchpad, String>>,
    },
    /// Retourne un snapshot du state complet.
    GetState {
        reply: oneshot::Sender<ProjectStateSnapshot>,
    },
    /// Demande si le projet est aborted (court-circuit rapide).
    IsAborted { reply: oneshot::Sender<bool> },
    /// Enregistre un plan après intercept_plan.
    RecordPlan { plan: FractalPlan },
    /// Enregistre le résultat d'un job après sync_reflect.
    RecordJobResult { result: JobResult },
    /// Marque le projet comme aborted.
    MarkAborted { reason: String, timestamp_ms: i64 },
    /// Reset l'état aborted (pour recovery test).
    ClearAborted,
    /// Compte les jobs par status.
    JobsByStatus {
        reply: oneshot::Sender<std::collections::HashMap<String, usize>>,
    },
    /// Shutdown l'actor.
    Shutdown,
}

/// Handle used by external code to talk to a ProjectActor.
#[derive(Debug, Clone)]
pub struct ProjectActorHandle {
    tx: mpsc::Sender<ProjectMessage>,
    cancel_token: CancellationToken,
}

impl ProjectActorHandle {
    /// Retrieve a clone of the current scratchpad (read-only snapshot, legacy).
    pub async fn get_scratchpad(&self) -> Result<cortex_core::Scratchpad, String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(ProjectMessage::GetScratchpad { reply: reply_tx })
            .await
            .map_err(|_| "actor channel closed".to_string())?;
        reply_rx
            .await
            .map_err(|_| "reply channel dropped".to_string())?
    }

    /// Snapshot complet du state.
    pub async fn get_state(&self) -> Result<ProjectStateSnapshot, String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(ProjectMessage::GetState { reply: reply_tx })
            .await
            .map_err(|_| "actor channel closed".to_string())?;
        reply_rx
            .await
            .map_err(|_| "reply channel dropped".to_string())
    }

    /// Demande si le projet est aborted.
    pub async fn is_aborted(&self) -> Result<bool, String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(ProjectMessage::IsAborted { reply: reply_tx })
            .await
            .map_err(|_| "actor channel closed".to_string())?;
        reply_rx
            .await
            .map_err(|_| "reply channel dropped".to_string())
    }

    /// Enregistre un plan.
    pub async fn record_plan(&self, plan: FractalPlan) -> Result<(), String> {
        self.tx
            .send(ProjectMessage::RecordPlan { plan })
            .await
            .map_err(|_| "actor channel closed".to_string())
    }

    /// Enregistre un job result.
    pub async fn record_job_result(&self, result: JobResult) -> Result<(), String> {
        self.tx
            .send(ProjectMessage::RecordJobResult { result })
            .await
            .map_err(|_| "actor channel closed".to_string())
    }

    /// Marque aborted.
    pub async fn mark_aborted(&self, reason: String, timestamp_ms: i64) -> Result<(), String> {
        self.tx
            .send(ProjectMessage::MarkAborted {
                reason,
                timestamp_ms,
            })
            .await
            .map_err(|_| "actor channel closed".to_string())
    }

    /// Reset aborted.
    pub async fn clear_aborted(&self) -> Result<(), String> {
        self.tx
            .send(ProjectMessage::ClearAborted)
            .await
            .map_err(|_| "actor channel closed".to_string())
    }

    /// Compte les jobs par status.
    pub async fn jobs_by_status(&self) -> Result<std::collections::HashMap<String, usize>, String> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(ProjectMessage::JobsByStatus { reply: reply_tx })
            .await
            .map_err(|_| "actor channel closed".to_string())?;
        reply_rx
            .await
            .map_err(|_| "reply channel dropped".to_string())
    }

    /// Signal the actor to shut down.
    pub fn shutdown(&self) {
        self.cancel_token.cancel();
    }
}

/// The actor itself. Runs in its own tokio task.
pub struct ProjectActor {
    #[allow(dead_code)]
    project_id: String,
    state: ProjectState,
    receiver: mpsc::Receiver<ProjectMessage>,
    cancel_token: CancellationToken,
}

impl ProjectActor {
    /// Spawn a new ProjectActor with an initial state.
    pub fn spawn(project_id: String, state: ProjectState) -> ProjectActorHandle {
        let (tx, rx) = mpsc::channel(256);
        let cancel_token = CancellationToken::new();
        let mut actor = Self {
            project_id: project_id.clone(),
            state,
            receiver: rx,
            cancel_token: cancel_token.clone(),
        };
        tokio::spawn(async move {
            actor.run().await;
        });
        ProjectActorHandle { tx, cancel_token }
    }

    /// Convenience: spawn from scratchpad (legacy compat).
    pub fn spawn_from_scratchpad(project_id: String, scratchpad: Scratchpad) -> ProjectActorHandle {
        let state = ProjectState {
            project_id: project_id.clone(),
            scratchpad,
            plan: None,
            job_results: Default::default(),
            audit_history: Vec::new(),
            aborted: false,
            aborted_at_ms: None,
            aborted_reason: None,
        };
        Self::spawn(project_id, state)
    }

    async fn run(&mut self) {
        loop {
            tokio::select! {
                _ = self.cancel_token.cancelled() => {
                    tracing::info!(project_id = %self.project_id, "ProjectActor shutting down");
                    break;
                }
                msg_opt = self.receiver.recv() => {
                    let Some(msg) = msg_opt else { break };
                    self.handle_message(msg).await;
                }
            }
        }
    }

    async fn handle_message(&mut self, msg: ProjectMessage) {
        match msg {
            ProjectMessage::GetScratchpad { reply } => {
                let _ = reply.send(Ok(self.state.scratchpad.clone()));
            }
            ProjectMessage::GetState { reply } => {
                let _ = reply.send(self.snapshot());
            }
            ProjectMessage::IsAborted { reply } => {
                let _ = reply.send(self.state.aborted);
            }
            ProjectMessage::RecordPlan { plan } => {
                self.state.plan = Some(plan);
            }
            ProjectMessage::RecordJobResult { result } => {
                self.state.record_job_result(result.clone());
                if let Some(audit) = result.last_audit {
                    self.state.record_audit(audit);
                }
            }
            ProjectMessage::MarkAborted {
                reason,
                timestamp_ms,
            } => {
                self.state.mark_aborted(reason, timestamp_ms);
            }
            ProjectMessage::ClearAborted => {
                self.state.aborted = false;
                self.state.aborted_reason = None;
                self.state.aborted_at_ms = None;
            }
            ProjectMessage::JobsByStatus { reply } => {
                let _ = reply.send(self.state.jobs_by_status());
            }
            ProjectMessage::Shutdown => {
                self.cancel_token.cancel();
            }
        }
    }

    fn snapshot(&self) -> ProjectStateSnapshot {
        let (themes_count, tasks_count) = match &self.state.plan {
            Some(p) => (p.themes.len(), p.themes.iter().map(|t| t.tasks.len()).sum()),
            None => (0, 0),
        };
        ProjectStateSnapshot {
            project_id: self.state.project_id.clone(),
            has_plan: self.state.plan.is_some(),
            plan_themes_count: themes_count,
            plan_total_tokens: tasks_count as u32,
            job_results: self.state.job_results.clone(),
            audit_history: self.state.audit_history.clone(),
            aborted: self.state.aborted,
            aborted_at_ms: self.state.aborted_at_ms,
            aborted_reason: self.state.aborted_reason.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project_state::{AuditRecord, JobResult};

    fn test_state() -> ProjectState {
        ProjectState::new("p1".to_string())
    }

    #[tokio::test]
    async fn test_project_actor_get_scratchpad() {
        let handle = ProjectActor::spawn("p1".to_string(), test_state());
        let pad = handle
            .get_scratchpad()
            .await
            .expect("should get scratchpad");
        assert_eq!(pad.project_id, "p1");
        handle.shutdown();
    }

    #[tokio::test]
    async fn test_project_actor_is_aborted_default_false() {
        let handle = ProjectActor::spawn("p2".to_string(), test_state());
        assert!(!handle.is_aborted().await.unwrap());
        handle.shutdown();
    }

    #[tokio::test]
    async fn test_project_actor_mark_aborted() {
        let handle = ProjectActor::spawn("p3".to_string(), test_state());
        handle.mark_aborted("user".into(), 1000).await.unwrap();
        assert!(handle.is_aborted().await.unwrap());
        let snap = handle.get_state().await.unwrap();
        assert_eq!(snap.aborted_reason.as_deref(), Some("user"));
        assert_eq!(snap.aborted_at_ms, Some(1000));
        handle.shutdown();
    }

    #[tokio::test]
    async fn test_project_actor_clear_aborted() {
        let handle = ProjectActor::spawn("p4".to_string(), test_state());
        handle.mark_aborted("oops".into(), 100).await.unwrap();
        assert!(handle.is_aborted().await.unwrap());
        handle.clear_aborted().await.unwrap();
        assert!(!handle.is_aborted().await.unwrap());
        handle.shutdown();
    }

    #[tokio::test]
    async fn test_project_actor_record_job_result() {
        let handle = ProjectActor::spawn("p5".to_string(), test_state());
        let result = JobResult {
            job_id: "J-1".into(),
            status: "committed".into(),
            last_action: "commit".into(),
            last_audit: Some(AuditRecord {
                job_id: "J-1".into(),
                passed: true,
                action: "commit".into(),
                timestamp_ms: 5000,
                issues_count: 0,
            }),
        };
        handle.record_job_result(result).await.unwrap();
        let counts = handle.jobs_by_status().await.unwrap();
        assert_eq!(counts.get("committed"), Some(&1));
        let snap = handle.get_state().await.unwrap();
        assert_eq!(snap.audit_history.len(), 1);
        handle.shutdown();
    }

    #[tokio::test]
    async fn test_project_actor_isolation() {
        // 2 actors séparés = state vraiment isolé
        let h1 = ProjectActor::spawn("proj-A".to_string(), test_state());
        let h2 = ProjectActor::spawn("proj-B".to_string(), test_state());
        h1.mark_aborted("A reason".into(), 1000).await.unwrap();
        assert!(h1.is_aborted().await.unwrap());
        assert!(!h2.is_aborted().await.unwrap()); // ← isolé !
        let snap_b = h2.get_state().await.unwrap();
        assert!(!snap_b.aborted);
        h1.shutdown();
        h2.shutdown();
    }
}
