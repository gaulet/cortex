//! # cortex-webhooks
//!
//! Système de notifications HTTP sortantes (webhooks) pour Cortex MCP.
//!
//! # Architecture
//!
//! ```text
//! CortexServer handler
//!       │
//! │ fire(event)│ (non-bloquant)
//!       ▼
//! WebhookDispatcher
//!       │
//! │ tokio::spawn│ (fire-and-forget)
//!       ▼
//! HTTP POST × retry(3, expo backoff) → URLs configurées
//! ```
//!
//! # Configuration
//!
//! Via env vars (lues par `CortexServer`) :
//! - `CORTEX_WEBHOOK_URLS` : URLs séparées par des virgules
//! - `CORTEX_WEBHOOK_TIMEOUT` : timeout HTTP en secondes (défaut 5)
//! - `CORTEX_WEBHOOK_MAX_RETRIES` : nombre de retries (défaut 3)
//! - `CORTEX_WEBHOOK_HMAC_SECRET` : secret pour signer les payloads (optionnel)
//!
//! # Événements supportés
//!
//! - `plan_generated` : après `intercept_plan` réussi
//! - `pre_mortem_emitted` : après `pre_mortem` réussi
//! - `recovery_triggered` : après `recover_project` avec entrées à traiter
//! - `audit_failed` : après `red_team_audit` qui bloque
//! - `abort` : après `abort` (emergency stop)
//!
//! # Sémantique de livraison
//!
//! - **At-least-once** : on retry jusqu'à 3 fois, donc le récepteur peut recevoir
//!   un événement plusieurs fois (idempotency côté récepteur)
//! - **Non-bloquant** : `fire()` retourne immédiatement, l'envoi tourne en background
//! - **Best-effort** : si toutes les retries échouent, on logge une erreur mais
//!   on n'impacte pas le flux principal de Cortex
//!
//! # Format du payload
//!
//! ```json
//! {
//!   "event": "plan_generated",
//!   "timestamp": "2026-06-04T12:00:00Z",
//!   "project_id": "project-...",
//!   "data": { /* event-specific */ },
//!   "delivery_id": "wh_...",
//!   "signature": "hmac_sha256=..."  // si HMAC configuré
//! }
//! ```

#![warn(missing_docs)]

mod dispatcher;
mod event;
mod config;

pub use dispatcher::WebhookDispatcher;
pub use event::{WebhookEvent, WebhookPayload};
pub use config::WebhookConfig;
