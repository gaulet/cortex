//! WebhookDispatcher : fire-and-forget HTTP POST avec retry exponentiel.

use crate::config::WebhookConfig;
use crate::event::{WebhookEvent, WebhookPayload};
use hmac::{Hmac, Mac};
use reqwest::Client;
use serde_json::Value as JsonValue;
use sha2::Sha256;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error, info, warn};

/// Dispatcher de webhooks. Clone-able et Send (utilisable dans tout le code async).
#[derive(Clone)]
pub struct WebhookDispatcher {
    inner: Arc<Inner>,
}

struct Inner {
    config: WebhookConfig,
    http: Client,
}

impl WebhookDispatcher {
    /// Crée un nouveau dispatcher. Retourne `None` si la config est vide.
    pub fn new(config: WebhookConfig) -> Option<Self> {
        if !config.is_enabled() {
            return None;
        }
        let http = Client::builder()
            .timeout(config.timeout)
            .build()
            .expect("reqwest client should build with default config");
        Some(Self {
            inner: Arc::new(Inner { config, http }),
        })
    }

    /// Émet un événement de manière fire-and-forget.
    ///
    /// Spawn une tokio task qui fait le POST + retry. Retourne immédiatement.
    /// Aucune erreur n'est remontée au caller (best-effort).
    pub fn fire(&self, event: WebhookEvent, project_id: Option<String>, data: JsonValue) {
        let payload = WebhookPayload::new(event, project_id, data);
        let dispatcher = self.clone();
        tokio::spawn(async move {
            dispatcher.deliver(payload).await;
        });
    }

    /// Variante synchrone (attend la livraison complète). Utile pour les tests.
    pub async fn fire_blocking(
        &self,
        event: WebhookEvent,
        project_id: Option<String>,
        data: JsonValue,
    ) -> Result<(), WebhookError> {
        let payload = WebhookPayload::new(event, project_id, data);
        self.deliver(payload).await
    }

    /// Retourne la liste des URLs configurées.
    pub fn urls(&self) -> Vec<String> {
        self.inner.config.urls.clone()
    }

    /// Retourne le nombre d'URLs configurées.
    pub fn url_count(&self) -> usize {
        self.inner.config.urls.len()
    }

    /// Retourne `true` si un secret HMAC est configuré.
    pub fn has_hmac(&self) -> bool {
        self.inner.config.hmac_secret.is_some()
    }

    // ============================================================
    // Internals
    // ============================================================

    async fn deliver(&self, mut payload: WebhookPayload) -> Result<(), WebhookError> {
        // 1. Signe le payload si HMAC configuré
        if let Some(secret) = &self.inner.config.hmac_secret {
            payload.signature = Some(compute_hmac(secret, &payload)?);
        }

        // 2. Sérialise une seule fois
        let body = serde_json::to_string(&payload).map_err(WebhookError::Serialize)?;

        // 3. Envoie à chaque URL avec retry
        let mut last_err: Option<WebhookError> = None;
        for url in &self.inner.config.urls {
            match self.send_with_retry(url, &body, &payload).await {
                Ok(()) => {
                    debug!(target: "cortex_webhooks", event = %payload.event, url = %url, "delivered");
                }
                Err(e) => {
                    error!(target: "cortex_webhooks", event = %payload.event, url = %url, error = %e, "delivery failed after all retries");
                    last_err = Some(e);
                }
            }
        }

        // Si au moins une URL a réussi, on n'échoue pas
        // (at-least-once → on veut la majorité des livraisons)
        match last_err {
            Some(e) if self.inner.config.urls.len() == 1 => Err(e),
            _ => Ok(()),
        }
    }

    async fn send_with_retry(
        &self,
        url: &str,
        body: &str,
        payload: &WebhookPayload,
    ) -> Result<(), WebhookError> {
        let max_attempts = self.inner.config.max_retries + 1;
        let mut backoff = self.inner.config.initial_backoff;

        for attempt in 1..=max_attempts {
            match self.send_once(url, body, payload).await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    if attempt < max_attempts {
                        warn!(target: "cortex_webhooks", event = %payload.event, url = %url, attempt, error = %e, "retrying");
                        tokio::time::sleep(backoff).await;
                        backoff = backoff.saturating_mul(2);
                    } else {
                        return Err(e);
                    }
                }
            }
        }
        unreachable!("loop always returns or continues")
    }

    async fn send_once(
        &self,
        url: &str,
        body: &str,
        payload: &WebhookPayload,
    ) -> Result<(), WebhookError> {
        let req = self
            .inner
            .http
            .post(url)
            .header("Content-Type", "application/json")
            .header("X-Cortex-Event", &payload.event)
            .header("X-Cortex-Delivery-Id", &payload.delivery_id)
            .header("X-Cortex-Timestamp", payload.timestamp.to_rfc3339())
            .body(body.to_string());

        let resp = req.send().await.map_err(WebhookError::Http)?;
        let status = resp.status();
        if status.is_success() {
            Ok(())
        } else {
            let text = resp.text().await.unwrap_or_default();
            Err(WebhookError::HttpStatus(status.as_u16(), text))
        }
    }
}

/// Erreurs possibles lors d'une livraison webhook.
#[derive(Debug, thiserror::Error)]
pub enum WebhookError {
    /// Sérialisation JSON échouée (ne devrait pas arriver).
    #[error("serialize: {0}")]
    Serialize(#[source] serde_json::Error),
    /// Erreur HTTP transport (timeout, connection refused, etc.).
    #[error("http: {0}")]
    Http(#[source] reqwest::Error),
    /// Réponse HTTP non-2xx.
    #[error("http status {0}: {1}")]
    HttpStatus(u16, String),
}

/// Signe un payload avec HMAC-SHA256, retourne le format `hmac_sha256=<hex>`.
fn compute_hmac(secret: &str, payload: &WebhookPayload) -> Result<String, WebhookError> {
    let body = serde_json::to_string(payload).map_err(WebhookError::Serialize)?;
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(secret.as_bytes())
        .map_err(|e| WebhookError::Serialize(serde_json::Error::io(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))))?;
    mac.update(body.as_bytes());
    let result = mac.finalize();
    Ok(format!("hmac_sha256={}", hex::encode(result.into_bytes())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WebhookConfig;

    #[test]
    fn test_new_returns_none_if_disabled() {
        let cfg = WebhookConfig::from_urls(vec![]);
        assert!(WebhookDispatcher::new(cfg).is_none());
    }

    #[test]
    fn test_new_returns_some_if_enabled() {
        let cfg = WebhookConfig::from_urls(vec!["https://example.com/wh".to_string()]);
        let d = WebhookDispatcher::new(cfg).expect("should be enabled");
        assert_eq!(d.url_count(), 1);
        assert!(!d.has_hmac());
    }

    #[test]
    fn test_urls_exposed() {
        let urls = vec!["https://a.com/wh".to_string(), "https://b.com/wh".to_string()];
        let cfg = WebhookConfig::from_urls(urls.clone());
        let d = WebhookDispatcher::new(cfg).expect("enabled");
        assert_eq!(d.urls(), urls);
    }

    #[test]
    fn test_hmac_detection() {
        let mut cfg = WebhookConfig::from_urls(vec!["https://example.com".to_string()]);
        assert!(!WebhookDispatcher::new(cfg.clone()).unwrap().has_hmac());
        cfg.hmac_secret = Some("my-secret".to_string());
        assert!(WebhookDispatcher::new(cfg).unwrap().has_hmac());
    }

    #[test]
    fn test_compute_hmac_format() {
        let payload = WebhookPayload::new(
            WebhookEvent::PlanGenerated,
            Some("p1".into()),
            serde_json::json!({"x": 1}),
        );
        let sig = compute_hmac("secret", &payload).expect("hmac");
        assert!(sig.starts_with("hmac_sha256="));
        // 64 hex chars = 32 bytes (SHA-256)
        let hex_part = sig.trim_start_matches("hmac_sha256=");
        assert_eq!(hex_part.len(), 64);
    }
}
