//! # cortex-actors
//!
//! Actor Model implementation for Cortex MCP using tokio.
//!
//! Each project gets its own `ProjectActor` that owns its state exclusively,
//! avoiding `Arc<Mutex<T>>` contention. Communication is done via typed channels
//! (mpsc + oneshot for replies).
//!
//! ## Actors
//!
//! - **ProjectActor** : Manages a single project (plan, jobs, audit history)
//! - **LlmActor** : Rate-limited LLM client
//! - **HermesClient** : Communicates with Hermes via MCP
//! - **ActorRegistry** : Registry of all project actors
//!
//! ## State ownership
//!
//! Each `ProjectActor` owns a `ProjectState` (see `project_state.rs`).
//! State is NEVER shared between actors — cross-project communication goes
//! through the shared `WalService` (filtered by `project_id`).

pub mod hermes_client;
pub mod llm_actor;
pub mod project_actor;
pub mod project_state;
pub mod registry;
pub mod shutdown;

pub use project_actor::{ProjectActor, ProjectActorHandle, ProjectMessage};
pub use project_state::{AuditRecord, JobResult, ProjectState};
pub use registry::ActorRegistry;
