//! Project actor : one actor per project, owns its Scratchpad exclusively.
//!
//! Communication is done via typed messages (mpsc) + oneshot reply channels,
//! avoiding Arc<Mutex<T>> contention entirely.

use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use cortex_core::{Scratchpad, Result};

/// Inbound messages a ProjectActor can handle.
#[derive(Debug)]
pub enum ProjectMessage {
    GetScratchpad {
        reply: oneshot::Sender<Result<Scratchpad>>,
    },
    Shutdown,
}

/// Handle used by external code to talk to a ProjectActor.
#[derive(Debug, Clone)]
pub struct ProjectActorHandle {
    tx: mpsc::Sender<ProjectMessage>,
    cancel_token: CancellationToken,
}

impl ProjectActorHandle {
    /// Retrieve a clone of the current scratchpad (read-only snapshot).
    pub async fn get_scratchpad(&self) -> Result<Scratchpad> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(ProjectMessage::GetScratchpad { reply: reply_tx })
            .await
            .map_err(|_| cortex_core::CortexError::InternalError("actor channel closed".into()))?;
        reply_rx
            .await
            .map_err(|_| cortex_core::CortexError::InternalError("reply channel dropped".into()))?
    }

    /// Signal the actor to shut down.
    pub fn shutdown(&self) {
        self.cancel_token.cancel();
    }
}

/// The actor itself. Runs in its own tokio task.
pub struct ProjectActor {
    project_id: String,
    scratchpad: Scratchpad,
    receiver: mpsc::Receiver<ProjectMessage>,
    cancel_token: CancellationToken,
}

impl ProjectActor {
    /// Spawn a new ProjectActor in the given runtime and return a handle.
    pub fn spawn(project_id: String, scratchpad: Scratchpad) -> ProjectActorHandle {
        let (tx, rx) = mpsc::channel(256);
        let cancel_token = CancellationToken::new();
        let mut actor = Self {
            project_id: project_id.clone(),
            scratchpad,
            receiver: rx,
            cancel_token: cancel_token.clone(),
        };
        tokio::spawn(async move {
            actor.run().await;
        });
        ProjectActorHandle { tx, cancel_token }
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
                let _ = reply.send(Ok(self.scratchpad.clone()));
            }
            ProjectMessage::Shutdown => {
                self.cancel_token.cancel();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_project_actor_get_scratchpad() {
        let pad = Scratchpad::new("p1".into(), "n".into(), "o".into()).unwrap();
        let handle = ProjectActor::spawn("p1".into(), pad);
        let snapshot = handle.get_scratchpad().await.expect("should get");
        assert_eq!(snapshot.project_id, "p1");
        handle.shutdown();
    }
}
