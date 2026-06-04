//! Configuration des webhooks (lue depuis env vars ou explicite).

use std::time::Duration;

/// Configuration du dispatcher de webhooks.
#[derive(Debug, Clone)]
pub struct WebhookConfig {
    /// URLs cibles (au moins une requise pour activer le dispatcher).
    pub urls: Vec<String>,
    /// Timeout par tentative HTTP.
    pub timeout: Duration,
    /// Nombre de retries max (en plus de la première tentative).
    pub max_retries: u32,
    /// Délai initial entre les retries (multiplié par 2 à chaque échec).
    pub initial_backoff: Duration,
    /// Secret HMAC pour signer les payloads (optionnel).
    pub hmac_secret: Option<String>,
}

impl WebhookConfig {
    /// Crée une config depuis une liste d'URLs (avec défauts).
    pub fn from_urls(urls: Vec<String>) -> Self {
        Self {
            urls,
            timeout: Duration::from_secs(5),
            max_retries: 3,
            initial_backoff: Duration::from_millis(500),
            hmac_secret: None,
        }
    }

    /// Builder : override le timeout par requête.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Builder : override le nombre max de retries.
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Lit la config depuis l'environnement.
    ///
    /// Variables :
    /// - `CORTEX_WEBHOOK_URLS` : URLs séparées par `,` (si vide/absent → dispatcher désactivé)
    /// - `CORTEX_WEBHOOK_TIMEOUT` : timeout en secondes (défaut 5)
    /// - `CORTEX_WEBHOOK_MAX_RETRIES` : nombre de retries (défaut 3)
    /// - `CORTEX_WEBHOOK_HMAC_SECRET` : secret HMAC (optionnel)
    pub fn from_env() -> Self {
        let urls: Vec<String> = std::env::var("CORTEX_WEBHOOK_URLS")
            .ok()
            .map(|s| {
                s.split(',')
                    .map(|u| u.trim().to_string())
                    .filter(|u| !u.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        let timeout_secs: u64 = std::env::var("CORTEX_WEBHOOK_TIMEOUT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(5);

        let max_retries: u32 = std::env::var("CORTEX_WEBHOOK_MAX_RETRIES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(3);

        let hmac_secret = std::env::var("CORTEX_WEBHOOK_HMAC_SECRET")
            .ok()
            .filter(|s| !s.is_empty());

        Self {
            urls,
            timeout: Duration::from_secs(timeout_secs),
            max_retries,
            initial_backoff: Duration::from_millis(500),
            hmac_secret,
        }
    }

    /// Retourne `true` si le dispatcher doit être actif.
    pub fn is_enabled(&self) -> bool {
        !self.urls.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_urls_enables_dispatcher() {
        let cfg = WebhookConfig::from_urls(vec!["https://example.com/wh".to_string()]);
        assert!(cfg.is_enabled());
        assert_eq!(cfg.max_retries, 3);
        assert_eq!(cfg.timeout, Duration::from_secs(5));
    }

    #[test]
    fn test_empty_urls_disables_dispatcher() {
        let cfg = WebhookConfig::from_urls(vec![]);
        assert!(!cfg.is_enabled());
    }

    #[test]
    fn test_from_env_no_vars() {
        // Pas de set env → URLs vide → désactivé
        let cfg = WebhookConfig::from_env();
        assert!(!cfg.is_enabled());
    }
}
