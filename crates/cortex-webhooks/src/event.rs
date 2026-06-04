//! Événements webhook.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use uuid::Uuid;

/// Les 5 événements critiques que Cortex peut émettre.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WebhookEvent {
    /// Plan fractal généré (après `intercept_plan` réussi)
    PlanGenerated,
    /// Guardrails Pre-Mortem émis (après `pre_mortem` réussi)
    PreMortemEmitted,
    /// Recovery déclenché (post-crash, entrées à traiter)
    RecoveryTriggered,
    /// Audit Red-Team a bloqué (reject)
    AuditFailed,
    /// Abort (emergency stop) émis
    Abort,
}

impl WebhookEvent {
    /// Nom de l'événement tel qu'il apparaît dans le payload JSON.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::PlanGenerated => "plan_generated",
            Self::PreMortemEmitted => "pre_mortem_emitted",
            Self::RecoveryTriggered => "recovery_triggered",
            Self::AuditFailed => "audit_failed",
            Self::Abort => "abort",
        }
    }
}

/// Payload complet envoyé à chaque URL.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookPayload {
    /// Nom de l'événement
    pub event: String,
    /// Timestamp ISO-8601 de l'émission
    pub timestamp: DateTime<Utc>,
    /// ID du projet concerné (si applicable)
    pub project_id: Option<String>,
    /// Données spécifiques à l'événement (structure varie selon `event`)
    pub data: JsonValue,
    /// ID unique de cette livraison (pour idempotency côté récepteur)
    pub delivery_id: String,
    /// Signature HMAC-SHA256 (si configurée)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

impl WebhookPayload {
    /// Crée un payload basique. Les champs optionnels sont remplis par le caller.
    pub fn new(event: WebhookEvent, project_id: Option<String>, data: JsonValue) -> Self {
        Self {
            event: event.as_str().to_string(),
            timestamp: Utc::now(),
            project_id,
            data,
            delivery_id: format!("wh_{}", Uuid::now_v7()),
            signature: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_as_str() {
        assert_eq!(WebhookEvent::PlanGenerated.as_str(), "plan_generated");
        assert_eq!(
            WebhookEvent::PreMortemEmitted.as_str(),
            "pre_mortem_emitted"
        );
        assert_eq!(
            WebhookEvent::RecoveryTriggered.as_str(),
            "recovery_triggered"
        );
        assert_eq!(WebhookEvent::AuditFailed.as_str(), "audit_failed");
        assert_eq!(WebhookEvent::Abort.as_str(), "abort");
    }

    #[test]
    fn test_payload_new_sets_fields() {
        let event = WebhookEvent::PlanGenerated;
        let data = serde_json::json!({"themes_count": 3});
        let payload = WebhookPayload::new(event.clone(), Some("proj-1".into()), data.clone());
        assert_eq!(payload.event, "plan_generated");
        assert_eq!(payload.project_id, Some("proj-1".to_string()));
        assert_eq!(payload.data, data);
        assert!(payload.delivery_id.starts_with("wh_"));
        assert!(payload.signature.is_none());
    }

    #[test]
    fn test_payload_serialization() {
        let payload = WebhookPayload::new(
            WebhookEvent::Abort,
            Some("p".into()),
            serde_json::json!({"reason": "test"}),
        );
        let json = serde_json::to_string(&payload).expect("serialize");
        assert!(json.contains("\"event\":\"abort\""));
        assert!(json.contains("\"project_id\":\"p\""));
        assert!(json.contains("\"reason\":\"test\""));
        // signature absente → pas dans la sortie
        assert!(!json.contains("signature"));
    }

    #[test]
    fn test_payload_serialization_with_signature() {
        let mut payload =
            WebhookPayload::new(WebhookEvent::Abort, Some("p".into()), serde_json::json!({}));
        payload.signature = Some("hmac_sha256=deadbeef".to_string());
        let json = serde_json::to_string(&payload).expect("serialize");
        assert!(json.contains("signature"));
    }
}
