//! Actor registry : registry of all active ProjectActors.
//!
//! Uses DashMap for concurrent read/write access without contention.

use std::sync::Arc;

use dashmap::DashMap;
use uuid::Uuid;

use cortex_core::{Result, Scratchpad};

use crate::project_actor::{ProjectActor, ProjectActorHandle};

/// Registry of all active project actors.
#[derive(Clone)]
pub struct ActorRegistry {
    actors: Arc<DashMap<String, ProjectActorHandle>>,
}

impl ActorRegistry {
    pub fn new() -> Self {
        Self {
            actors: Arc::new(DashMap::new()),
        }
    }

    /// Get or create a ProjectActor for the given project_id.
    pub fn get_or_create(
        &self,
        project_id: &str,
        project_name: &str,
        objective: &str,
    ) -> Result<ProjectActorHandle> {
        if let Some(handle) = self.actors.get(project_id) {
            return Ok(handle.clone());
        }
        let scratchpad = Scratchpad::new(
            project_id.to_string(),
            project_name.to_string(),
            objective.to_string(),
        )?;
        let handle = ProjectActor::spawn_from_scratchpad(project_id.to_string(), scratchpad);
        self.actors.insert(project_id.to_string(), handle.clone());
        Ok(handle)
    }

    /// Remove an actor from the registry (e.g. after abort).
    pub fn remove(&self, project_id: &str) -> Option<ProjectActorHandle> {
        self.actors.remove(project_id).map(|(_, h)| h)
    }

    /// Number of active actors.
    pub fn count(&self) -> usize {
        self.actors.len()
    }

    /// Generate a unique project id (convenience helper).
    pub fn generate_project_id() -> String {
        format!("proj_{}", Uuid::now_v7())
    }
}

impl Default for ActorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_registry_get_or_create() {
        let registry = ActorRegistry::new();
        let h1 = registry.get_or_create("p1", "proj", "obj").unwrap();
        let h2 = registry.get_or_create("p1", "proj", "obj").unwrap();
        assert_eq!(registry.count(), 1);
        drop(h1);
        drop(h2);
    }

    #[tokio::test]
    async fn test_registry_multiple_projects() {
        let registry = ActorRegistry::new();
        registry.get_or_create("p1", "a", "o").unwrap();
        registry.get_or_create("p2", "b", "o").unwrap();
        assert_eq!(registry.count(), 2);
    }
}
