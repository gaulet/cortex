//! # cortex-brains
//!
//! LLM reasoning brains for Cortex MCP.
//!
//! Contains 4 specialized LLM brains:
//! - **Architect** : Fractal plan decomposition
//! - **Pre-Mortem** (Paranoiac) : Anticipate failures using Hermes F1-F19 catalog
//! - **Red-Team** (Adversarial) : Validate worker output
//! - **Insights Harvester** : Extract patterns/lessons from completed themes
//!
//! Also contains the cost-gating logic (§6.5 specification-cortex-v2.md).

pub mod architect;
pub mod cost_gating;
pub mod insights;
pub mod paranoiac;
pub mod red_team;

pub use architect::Architect;
pub use cost_gating::{activate_brains_for_job, CostGating};
pub use insights::InsightsHarvester;
pub use paranoiac::PreMortem;
pub use red_team::RedTeam;
