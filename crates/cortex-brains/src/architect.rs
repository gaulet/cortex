//! Architect brain : fractal plan decomposition.
//!
//! The Architect LLM takes a user intent + Hermes context and returns a
//! MECE-structured plan with themes, jobs, dependencies, criticity scores,
//! and convergence contracts.
//!
//! Placeholder for now. LLM call logic comes later in Phase 4.

/// Architect brain (generates fractal plans).
pub struct Architect;

impl Architect {
    pub fn new() -> Self {
        Self
    }
}

impl Default for Architect {
    fn default() -> Self {
        Self::new()
    }
}
