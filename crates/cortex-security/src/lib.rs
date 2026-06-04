//! # cortex-security
//!
//! Security utilities for Cortex MCP.
//!
//! - HMAC signature of job guardrails (anti-tampering)
//! - Schema validation for incoming contexts/contracts
//! - Signature verification in sync_reflect

pub mod hmac;
pub mod schema;

pub use hmac::{sign_guardrails, verify_guardrails};
