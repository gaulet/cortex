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
//!
//! All brains use the `LlmClient` trait, enabling injection of either a real
//! HTTP client (reqwest) or a mock for tests.

pub mod architect;
pub mod cost_gating;
pub mod http_llm_client;
pub mod insights;
pub mod llm_client;
pub mod paranoiac;
pub mod red_team;

pub use architect::{Architect, ArchitectError, FractalPlan, PlannedTask, PlannedTheme};
pub use cost_gating::{activate_brains_for_job, CostGating};
pub use http_llm_client::HttpLlmClient;
pub use insights::InsightsHarvester;
pub use llm_client::{LlmClient, LlmError, LlmRequest, LlmResponse, MockLlmClient};
pub use paranoiac::PreMortem;
pub use red_team::RedTeam;
