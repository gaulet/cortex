//! Context packets exchanged with Hermes.
//!
//! A `ContextPacket` is the envelope Hermes sends to Cortex for every MCP call.
//! It contains the user intent, the conversation delta, and any triggering event.

use serde::{Deserialize, Serialize};

/// Envelope Hermes sends to Cortex on every MCP call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextPacket {
    pub packet_id: String,
    pub user_intent: String,
    pub conversation_delta: Vec<String>,
    pub active_project_hint: Option<String>,
    pub triggering_event: Option<TriggeringEvent>,
}

/// Event that triggered the current MCP call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggeringEvent {
    pub event_type: String,
    pub job_id: Option<String>,
}
