//! Dependency graph operations between themes.
//!
//! Used by the Architect brain to validate plans and by Cortex to check
//! convergence contracts before unblocking dependent themes.
//!
//! Placeholder for now. Full implementation in Phase 4.

/// Graph structure representing theme dependencies.
#[derive(Debug, Clone, Default)]
pub struct DependencyGraph {
    /// theme_id -> list of themes it depends on
    pub edges: std::collections::HashMap<String, Vec<String>>,
}

impl DependencyGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_dependency(&mut self, from: String, to: String) {
        self.edges.entry(from).or_default().push(to);
    }
}
