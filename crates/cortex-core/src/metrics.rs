//! Compteurs et jauges pour observabilité Prometheus.
//!
//! Le serveur Cortex expose ses métriques au format Prometheus (text/plain)
//! via l'outil MCP `get_metrics`. Les compteurs sont stockés en `AtomicU64`
//! pour permettre des updates lock-free depuis n'importe quel task async.
//!
//! # Métriques exposées
//!
//! - `cortex_jobs_dispatched_total` (counter) : nombre de jobs dispatchés
//! - `cortex_jobs_approved_total` (counter) : jobs approuvés par Hermes
//! - `cortex_jobs_rejected_total` (counter) : jobs rejetés par Red-Team
//! - `cortex_jobs_escalated_total` (counter) : jobs escaladés humainement
//! - `cortex_red_team_blocks_total` (counter) : audits Red-Team qui ont bloqué
//! - `cortex_pre_mortem_guards_emitted_total` (counter) : guardrails émis
//! - `cortex_hmac_verifications_total{result=ok|fail}` (counter) : HMAC verifs
//! - `cortex_recovery_rolled_back_total` (counter) : entries rolled back
//! - `cortex_recovery_escalated_total` (counter) : entries escalated
//! - `cortex_llm_requests_total{provider=mim|mock}` (counter) : appels LLM
//! - `cortex_llm_tokens_consumed_total` (counter) : tokens cumulés
//! - `cortex_uptime_seconds` (gauge) : uptime du serveur
//!
//! Note : pas d'histogrammes dans ce MVP (overhead AtomicI64). On expose
//! des percentiles calculables côté Prometheus via recording rules.

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// Compteurs partagés (lock-free).
#[derive(Debug, Default)]
pub struct Metrics {
    pub jobs_dispatched: AtomicU64,
    pub jobs_approved: AtomicU64,
    pub jobs_rejected: AtomicU64,
    pub jobs_escalated: AtomicU64,
    pub red_team_blocks: AtomicU64,
    pub pre_mortem_guards_emitted: AtomicU64,
    pub hmac_verifications_ok: AtomicU64,
    pub hmac_verifications_fail: AtomicU64,
    pub recovery_rolled_back: AtomicU64,
    pub recovery_escalated: AtomicU64,
    pub llm_requests_total: AtomicU64,
    pub llm_tokens_consumed: AtomicU64,
    pub start_time: AtomicI64, // unix timestamp millis du boot
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            start_time: AtomicI64::new(chrono::Utc::now().timestamp_millis()),
            ..Default::default()
        }
    }

    pub fn inc_jobs_dispatched(&self) {
        self.jobs_dispatched.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_jobs_approved(&self) {
        self.jobs_approved.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_jobs_rejected(&self) {
        self.jobs_rejected.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_jobs_escalated(&self) {
        self.jobs_escalated.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_red_team_blocks(&self) {
        self.red_team_blocks.fetch_add(1, Ordering::Relaxed);
    }
    pub fn add_pre_mortem_guards(&self, n: u64) {
        self.pre_mortem_guards_emitted
            .fetch_add(n, Ordering::Relaxed);
    }
    pub fn inc_hmac_ok(&self) {
        self.hmac_verifications_ok.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_hmac_fail(&self) {
        self.hmac_verifications_fail.fetch_add(1, Ordering::Relaxed);
    }
    pub fn add_recovery_rolled_back(&self, n: u64) {
        self.recovery_rolled_back.fetch_add(n, Ordering::Relaxed);
    }
    pub fn add_recovery_escalated(&self, n: u64) {
        self.recovery_escalated.fetch_add(n, Ordering::Relaxed);
    }
    pub fn inc_llm_requests(&self) {
        self.llm_requests_total.fetch_add(1, Ordering::Relaxed);
    }
    pub fn add_llm_tokens(&self, n: u64) {
        self.llm_tokens_consumed.fetch_add(n, Ordering::Relaxed);
    }

    /// Sérialise les compteurs au format Prometheus (text/plain).
    pub fn to_prometheus(&self) -> String {
        let now = chrono::Utc::now().timestamp_millis();
        let start = self.start_time.load(Ordering::Relaxed);
        let uptime_secs = (now - start) / 1000;

        let mut out = String::with_capacity(2048);
        out.push_str("# HELP cortex_jobs_dispatched_total Total jobs dispatched\n");
        out.push_str("# TYPE cortex_jobs_dispatched_total counter\n");
        out.push_str(&format!(
            "cortex_jobs_dispatched_total {}\n",
            self.jobs_dispatched.load(Ordering::Relaxed)
        ));
        out.push_str("# HELP cortex_jobs_approved_total Total jobs approved\n");
        out.push_str("# TYPE cortex_jobs_approved_total counter\n");
        out.push_str(&format!(
            "cortex_jobs_approved_total {}\n",
            self.jobs_approved.load(Ordering::Relaxed)
        ));
        out.push_str("# HELP cortex_jobs_rejected_total Total jobs rejected\n");
        out.push_str("# TYPE cortex_jobs_rejected_total counter\n");
        out.push_str(&format!(
            "cortex_jobs_rejected_total {}\n",
            self.jobs_rejected.load(Ordering::Relaxed)
        ));
        out.push_str("# HELP cortex_jobs_escalated_total Total jobs escalated\n");
        out.push_str("# TYPE cortex_jobs_escalated_total counter\n");
        out.push_str(&format!(
            "cortex_jobs_escalated_total {}\n",
            self.jobs_escalated.load(Ordering::Relaxed)
        ));
        out.push_str("# HELP cortex_red_team_blocks_total Red-Team audits that blocked\n");
        out.push_str("# TYPE cortex_red_team_blocks_total counter\n");
        out.push_str(&format!(
            "cortex_red_team_blocks_total {}\n",
            self.red_team_blocks.load(Ordering::Relaxed)
        ));
        out.push_str("# HELP cortex_pre_mortem_guards_emitted_total Guardrails emitted\n");
        out.push_str("# TYPE cortex_pre_mortem_guards_emitted_total counter\n");
        out.push_str(&format!(
            "cortex_pre_mortem_guards_emitted_total {}\n",
            self.pre_mortem_guards_emitted.load(Ordering::Relaxed)
        ));
        out.push_str("# HELP cortex_hmac_verifications_total HMAC verifications\n");
        out.push_str("# TYPE cortex_hmac_verifications_total counter\n");
        out.push_str(&format!(
            "cortex_hmac_verifications_total{{result=\"ok\"}} {}\n",
            self.hmac_verifications_ok.load(Ordering::Relaxed)
        ));
        out.push_str(&format!(
            "cortex_hmac_verifications_total{{result=\"fail\"}} {}\n",
            self.hmac_verifications_fail.load(Ordering::Relaxed)
        ));
        out.push_str("# HELP cortex_recovery_rolled_back_total Recovery entries rolled back\n");
        out.push_str("# TYPE cortex_recovery_rolled_back_total counter\n");
        out.push_str(&format!(
            "cortex_recovery_rolled_back_total {}\n",
            self.recovery_rolled_back.load(Ordering::Relaxed)
        ));
        out.push_str("# HELP cortex_recovery_escalated_total Recovery entries escalated\n");
        out.push_str("# TYPE cortex_recovery_escalated_total counter\n");
        out.push_str(&format!(
            "cortex_recovery_escalated_total {}\n",
            self.recovery_escalated.load(Ordering::Relaxed)
        ));
        out.push_str("# HELP cortex_llm_requests_total LLM API calls\n");
        out.push_str("# TYPE cortex_llm_requests_total counter\n");
        out.push_str(&format!(
            "cortex_llm_requests_total {}\n",
            self.llm_requests_total.load(Ordering::Relaxed)
        ));
        out.push_str("# HELP cortex_llm_tokens_consumed_total LLM tokens consumed\n");
        out.push_str("# TYPE cortex_llm_tokens_consumed_total counter\n");
        out.push_str(&format!(
            "cortex_llm_tokens_consumed_total {}\n",
            self.llm_tokens_consumed.load(Ordering::Relaxed)
        ));
        out.push_str("# HELP cortex_uptime_seconds Server uptime\n");
        out.push_str("# TYPE cortex_uptime_seconds gauge\n");
        out.push_str(&format!("cortex_uptime_seconds {}\n", uptime_secs));
        out
    }
}

/// Wrapper thread-safe pour partager les metrics entre tasks.
pub type SharedMetrics = Arc<Metrics>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metrics_default_zero() {
        let m = Metrics::new();
        assert_eq!(m.jobs_dispatched.load(Ordering::Relaxed), 0);
        assert_eq!(m.jobs_approved.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn test_metrics_incrementers() {
        let m = Metrics::new();
        m.inc_jobs_dispatched();
        m.inc_jobs_dispatched();
        m.inc_jobs_approved();
        m.add_pre_mortem_guards(5);
        m.add_llm_tokens(1234);
        m.inc_hmac_fail();

        assert_eq!(m.jobs_dispatched.load(Ordering::Relaxed), 2);
        assert_eq!(m.jobs_approved.load(Ordering::Relaxed), 1);
        assert_eq!(m.pre_mortem_guards_emitted.load(Ordering::Relaxed), 5);
        assert_eq!(m.llm_tokens_consumed.load(Ordering::Relaxed), 1234);
        assert_eq!(m.hmac_verifications_fail.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_metrics_prometheus_format() {
        let m = Metrics::new();
        m.inc_jobs_dispatched();
        m.inc_red_team_blocks();
        let out = m.to_prometheus();
        assert!(out.contains("# TYPE cortex_jobs_dispatched_total counter"));
        assert!(out.contains("cortex_jobs_dispatched_total 1"));
        assert!(out.contains("cortex_red_team_blocks_total 1"));
        assert!(out.contains("# TYPE cortex_uptime_seconds gauge"));
        assert!(out.contains("cortex_uptime_seconds"));
    }

    #[test]
    fn test_metrics_shared_concurrent() {
        use std::thread;
        let m = Arc::new(Metrics::new());
        let mut handles = vec![];
        for _ in 0..10 {
            let m2 = m.clone();
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    m2.inc_jobs_dispatched();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(m.jobs_dispatched.load(Ordering::Relaxed), 1000);
    }
}
