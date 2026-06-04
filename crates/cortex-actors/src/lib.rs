//! # cortex-actors
//!
//! Actor Model implementation for Cortex MCP using tokio.
//!
//! Each project gets its own `ProjectActor` that owns its Scratchpad exclusively,
//! avoiding `Arc<Mutex<T>>` contention. Communication is done via typed channels
//! (mpsc + oneshot for replies).
//!
//! ## Actors
//!
//! - **ProjectActor** : Manages a single project (scratchpad, themes, jobs)
//! - **LlmActor** : Rate-limited LLM client
//! - **HermesClient** : Communicates with Hermes via MCP
//! - **ActorRegistry** : Registry of all project actors

pub mod hermes_client;
pub mod llm_actor;
pub mod project_actor;
pub mod registry;
pub mod shutdown;

pub use project_actor::{ProjectActor, ProjectActorHandle};
pub use registry::ActorRegistry;
