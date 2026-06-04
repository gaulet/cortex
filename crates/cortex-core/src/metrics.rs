//! Compteurs, jauges et histogrammes pour observabilité Prometheus.
//!
//! Le serveur Cortex expose ses métriques au format Prometheus (text/plain)
//! via l'outil MCP `get_metrics`. Tous les compteurs sont `AtomicU64` (lock-free)
//! et les histogrammes utilisent des buckets cumulatifs (lock-free aussi).
//!
//! # Métriques exposées (Session 5.3 + 6)
//!
//! ## Counters (13)
//!
//! - `cortex_jobs_dispatched_total` : nombre de jobs dispatchés
//! - `cortex_jobs_approved_total` : jobs approuvés par Hermes
//! - `cortex_jobs_rejected_total` : jobs rejetés par Red-Team
//! - `cortex_jobs_escalated_total` : jobs escaladés humainement
//! - `cortex_red_team_blocks_total` : audits Red-Team qui ont bloqué
//! - `cortex_pre_mortem_guards_emitted_total` : guardrails émis
//! - `cortex_hmac_verifications_total{result=ok|fail}` : HMAC verifs
//! - `cortex_recovery_rolled_back_total` : entries rolled back
//! - `cortex_recovery_escalated_total` : entries escalated
//! - `cortex_llm_requests_total` : appels LLM
//! - `cortex_llm_tokens_consumed_total` : tokens cumulés
//!
//! ## Gauge (1)
//!
//! - `cortex_uptime_seconds` : uptime du serveur
//!
//! # Histogrammes (Session 6 - option A)
//!
//! Latences en secondes, buckets logarithmiques 1ms → 30s.
//! Calculer p50/p95/p99 via `histogram_quantile()` en PromQL.
//!
//! - `cortex_intercept_plan_duration_seconds` : latence intercept_plan
//! - `cortex_pre_mortem_duration_seconds` : latence pre_mortem
//! - `cortex_red_team_audit_duration_seconds` : latence audit complet
//! - `cortex_sync_reflect_duration_seconds` : latence sync_reflect
//! - `cortex_approve_and_execute_duration_seconds` : latence dispatch
//! - `cortex_recover_project_duration_seconds` : latence recovery
//! - `cortex_harvest_insights_duration_seconds` : latence insights
//! - `cortex_llm_request_duration_seconds` : latence appels LLM bruts
//!
//! Note : pour SLO percentiles (p50/p95/p99), utiliser Prometheus
//! `histogram_quantile()` sur les buckets. Les buckets sont en secondes
//! avec une progression logarithmique (1ms → 30s).
//!
//! # Overhead
//!
//! - observe() : N atomic adds (N = buckets.len() = 15) + 1 sum + 1 count
//!   Pour 14 buckets × 10⁵ observes/s : ~1.6M atomic ops/s, négligeable.
//! - snapshot() : N atomic loads, négligeable.

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Buckets par défaut (secondes) pour tous les histogrammes.
/// Distribution logarithmique qui couvre 1ms → ~30s, inspirée de
/// `prometheus_client::DEFAULT_BUCKETS` (à peu près).
pub const DEFAULT_BUCKETS: &[f64] = &[
    0.001, 0.0025, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0,
];

/// Histogramme lock-free avec buckets cumulatifs et sum atomique.
///
/// # Sémantique cumulative
///
/// À chaque `observe(value)`, on incrémente TOUS les buckets où
/// `le >= value`. Donc `bucket[le=0.5]` compte toutes les observations ≤ 0.5s,
/// `bucket[le=+Inf]` compte TOUTES les observations (= count total).
///
/// C'est la sémantique Prometheus standard : `histogram_quantile(0.95, ...)`
/// interpole entre le bucket p95 et le suivant.
///
/// # Overhead par observe
///
/// - N atomic adds (N = buckets.len(), default 15)
/// - 1 atomic add (sum_micros)
/// - 1 atomic add (count)
/// → ~17 atomic ops/observe, négligeable (< 1µs sur x86).
pub struct Histogram {
    buckets: Vec<(f64, AtomicU64)>, // (upper_bound_seconds, cumulative count)
    sum_micros: AtomicI64,           // sum en microsecondes (i64::MAX = ~292 000 ans)
    count: AtomicU64,
}

impl Histogram {
    /// Crée un histogramme avec les buckets donnés (+ bucket +Inf ajouté).
    pub fn new(buckets: &[f64]) -> Self {
        let mut b: Vec<(f64, AtomicU64)> = buckets
            .iter()
            .map(|&le| (le, AtomicU64::new(0)))
            .collect();
        // Bucket +Inf : capture tout ce qui dépasse le max bucket
        b.push((f64::INFINITY, AtomicU64::new(0)));
        Self {
            buckets: b,
            sum_micros: AtomicI64::new(0),
            count: AtomicU64::new(0),
        }
    }

    /// Enregistre une observation de durée.
    pub fn observe(&self, duration: Duration) {
        let secs = duration.as_secs_f64();
        let micros = duration.as_micros() as i64;
        // Incrémenter tous les buckets où le <= obs (sémantique cumulative)
        for (le, count) in &self.buckets {
            if secs <= *le {
                count.fetch_add(1, Ordering::Relaxed);
            }
        }
        self.sum_micros.fetch_add(micros, Ordering::Relaxed);
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    /// Snapshot immutable (pour export Prometheus).
    pub fn snapshot(&self) -> HistogramSnapshot {
        let buckets: Vec<(f64, u64)> = self
            .buckets
            .iter()
            .map(|(le, c)| (*le, c.load(Ordering::Relaxed)))
            .collect();
        HistogramSnapshot {
            buckets,
            sum_micros: self.sum_micros.load(Ordering::Relaxed),
            count: self.count.load(Ordering::Relaxed),
        }
    }
}

impl Default for Histogram {
    fn default() -> Self {
        Self::new(DEFAULT_BUCKETS)
    }
}

// Note: Histogram ne peut pas être #[derive(Debug)] à cause de Vec<AtomicU64>.
// Mais on peut l'implémenter manuellement.
impl std::fmt::Debug for Histogram {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Histogram")
            .field("count", &self.count.load(Ordering::Relaxed))
            .field("sum_micros", &self.sum_micros.load(Ordering::Relaxed))
            .finish()
    }
}

/// Snapshot immutable d'un Histogram (pour export).
#[derive(Debug, Clone)]
pub struct HistogramSnapshot {
    pub buckets: Vec<(f64, u64)>, // (le, cumulative_count) — déjà cumulés dans observe()
    pub sum_micros: i64,
    pub count: u64,
}

impl HistogramSnapshot {
    /// Sérialise au format Prometheus :
    /// `name_bucket{le="0.001"} 5`
    /// `name_bucket{le="0.01"} 12`
    /// ...
    /// `name_bucket{le="+Inf"} 20`
    /// `name_sum 0.123`
    /// `name_count 20`
    pub fn to_prometheus(&self, name: &str, help: &str) -> String {
        let mut out = String::with_capacity(256);
        out.push_str(&format!("# HELP {} {}\n", name, help));
        out.push_str(&format!("# TYPE {} histogram\n", name));
        for (le, count) in &self.buckets {
            let le_str = if le.is_infinite() {
                "+Inf".to_string()
            } else {
                format!("{}", le)
            };
            out.push_str(&format!(
                "{}_bucket{{le=\"{}\"}} {}\n",
                name, le_str, count
            ));
        }
        // Sum en secondes (Prometheus convention)
        let sum_secs = self.sum_micros as f64 / 1_000_000.0;
        out.push_str(&format!("{}_sum {}\n", name, sum_secs));
        out.push_str(&format!("{}_count {}\n", name, self.count));
        out
    }
}

/// Timer guard : observe automatiquement la durée à la drop.
///
/// Usage :
/// ```ignore
/// let _timer = metrics.intercept_plan_duration.start_timer();
/// // ... do work ...
/// // timer.observe() appelé automatiquement à la drop
/// ```
pub struct HistogramTimer<'a> {
    histogram: &'a Histogram,
    start: Instant,
}

impl<'a> HistogramTimer<'a> {
    pub fn start(histogram: &'a Histogram) -> Self {
        Self {
            histogram,
            start: Instant::now(),
        }
    }

    /// Observe maintenant (avant la drop).
    pub fn observe(self) -> Duration {
        let elapsed = self.start.elapsed();
        self.histogram.observe(elapsed);
        elapsed
    }
}

impl<'a> Drop for HistogramTimer<'a> {
    fn drop(&mut self) {
        // Si l'utilisateur a oublié d'appeler observe(), on observe à la drop
        // pour ne perdre aucune donnée.
        let elapsed = self.start.elapsed();
        self.histogram.observe(elapsed);
    }
}

/// Extension trait pour démarrer un timer.
pub trait HistogramExt {
    fn start_timer(&self) -> HistogramTimer<'_>;
}

impl HistogramExt for Histogram {
    fn start_timer(&self) -> HistogramTimer<'_> {
        HistogramTimer::start(self)
    }
}

/// Compteurs et histogrammes partagés (lock-free).
#[derive(Debug)]
pub struct Metrics {
    // Counters (13)
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
    // Gauge (1)
    pub start_time: AtomicI64, // unix timestamp millis du boot
    // Histograms (8) — Session 6 option A
    pub intercept_plan_duration: Histogram,
    pub pre_mortem_duration: Histogram,
    pub red_team_audit_duration: Histogram,
    pub sync_reflect_duration: Histogram,
    pub approve_and_execute_duration: Histogram,
    pub recover_project_duration: Histogram,
    pub harvest_insights_duration: Histogram,
    pub llm_request_duration: Histogram,
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            jobs_dispatched: AtomicU64::new(0),
            jobs_approved: AtomicU64::new(0),
            jobs_rejected: AtomicU64::new(0),
            jobs_escalated: AtomicU64::new(0),
            red_team_blocks: AtomicU64::new(0),
            pre_mortem_guards_emitted: AtomicU64::new(0),
            hmac_verifications_ok: AtomicU64::new(0),
            hmac_verifications_fail: AtomicU64::new(0),
            recovery_rolled_back: AtomicU64::new(0),
            recovery_escalated: AtomicU64::new(0),
            llm_requests_total: AtomicU64::new(0),
            llm_tokens_consumed: AtomicU64::new(0),
            start_time: AtomicI64::new(chrono::Utc::now().timestamp_millis()),
            intercept_plan_duration: Histogram::default(),
            pre_mortem_duration: Histogram::default(),
            red_team_audit_duration: Histogram::default(),
            sync_reflect_duration: Histogram::default(),
            approve_and_execute_duration: Histogram::default(),
            recover_project_duration: Histogram::default(),
            harvest_insights_duration: Histogram::default(),
            llm_request_duration: Histogram::default(),
        }
    }

    // Counter incrementers
    pub fn inc_jobs_dispatched(&self) { self.jobs_dispatched.fetch_add(1, Ordering::Relaxed); }
    pub fn inc_jobs_approved(&self) { self.jobs_approved.fetch_add(1, Ordering::Relaxed); }
    pub fn inc_jobs_rejected(&self) { self.jobs_rejected.fetch_add(1, Ordering::Relaxed); }
    pub fn inc_jobs_escalated(&self) { self.jobs_escalated.fetch_add(1, Ordering::Relaxed); }
    pub fn inc_red_team_blocks(&self) { self.red_team_blocks.fetch_add(1, Ordering::Relaxed); }
    pub fn add_pre_mortem_guards(&self, n: u64) {
        self.pre_mortem_guards_emitted.fetch_add(n, Ordering::Relaxed);
    }
    pub fn inc_hmac_ok(&self) { self.hmac_verifications_ok.fetch_add(1, Ordering::Relaxed); }
    pub fn inc_hmac_fail(&self) { self.hmac_verifications_fail.fetch_add(1, Ordering::Relaxed); }
    pub fn add_recovery_rolled_back(&self, n: u64) { self.recovery_rolled_back.fetch_add(n, Ordering::Relaxed); }
    pub fn add_recovery_escalated(&self, n: u64) { self.recovery_escalated.fetch_add(n, Ordering::Relaxed); }
    pub fn inc_llm_requests(&self) { self.llm_requests_total.fetch_add(1, Ordering::Relaxed); }
    pub fn add_llm_tokens(&self, n: u64) { self.llm_tokens_consumed.fetch_add(n, Ordering::Relaxed); }

    /// Sérialise TOUTES les métriques (counters + gauge + 8 histogrammes)
    /// au format Prometheus (text/plain).
    pub fn to_prometheus(&self) -> String {
        let now = chrono::Utc::now().timestamp_millis();
        let start = self.start_time.load(Ordering::Relaxed);
        let uptime_secs = (now - start) / 1000;

        let mut out = String::with_capacity(4096);

        // ============ COUNTERS (13) ============
        let counters: [(&str, &str, u64); 12] = [
            ("cortex_jobs_dispatched_total", "Total jobs dispatched", self.jobs_dispatched.load(Ordering::Relaxed)),
            ("cortex_jobs_approved_total", "Total jobs approved", self.jobs_approved.load(Ordering::Relaxed)),
            ("cortex_jobs_rejected_total", "Total jobs rejected", self.jobs_rejected.load(Ordering::Relaxed)),
            ("cortex_jobs_escalated_total", "Total jobs escalated", self.jobs_escalated.load(Ordering::Relaxed)),
            ("cortex_red_team_blocks_total", "Red-Team audits that blocked", self.red_team_blocks.load(Ordering::Relaxed)),
            ("cortex_pre_mortem_guards_emitted_total", "Guardrails emitted", self.pre_mortem_guards_emitted.load(Ordering::Relaxed)),
            ("cortex_recovery_rolled_back_total", "Recovery entries rolled back", self.recovery_rolled_back.load(Ordering::Relaxed)),
            ("cortex_recovery_escalated_total", "Recovery entries escalated", self.recovery_escalated.load(Ordering::Relaxed)),
            ("cortex_llm_requests_total", "LLM API calls", self.llm_requests_total.load(Ordering::Relaxed)),
            ("cortex_llm_tokens_consumed_total", "LLM tokens consumed", self.llm_tokens_consumed.load(Ordering::Relaxed)),
            // Ces deux derniers sont des sous-catégories du même nom avec labels
            // → traités en dehors du tableau ci-dessous
            ("_placeholder1", "", 0),
            ("_placeholder2", "", 0),
        ];
        for (name, help, value) in &counters {
            if name.starts_with('_') { continue; }
            out.push_str(&format!("# HELP {} {}\n", name, help));
            out.push_str(&format!("# TYPE {} counter\n", name));
            out.push_str(&format!("{} {}\n", name, value));
        }

        // HMAC verifications : counter labellisé
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

        // ============ GAUGE (1) ============
        out.push_str("# HELP cortex_uptime_seconds Server uptime\n");
        out.push_str("# TYPE cortex_uptime_seconds gauge\n");
        out.push_str(&format!("cortex_uptime_seconds {}\n", uptime_secs));

        // ============ HISTOGRAMS (8) ============
        let histograms: [(&str, &str, &Histogram); 8] = [
            ("cortex_intercept_plan_duration_seconds",
             "Time spent in intercept_plan (Architect LLM call)",
             &self.intercept_plan_duration),
            ("cortex_pre_mortem_duration_seconds",
             "Time spent in pre_mortem (PreMortem LLM call)",
             &self.pre_mortem_duration),
            ("cortex_red_team_audit_duration_seconds",
             "Time spent in red_team_audit (5-layer audit)",
             &self.red_team_audit_duration),
            ("cortex_sync_reflect_duration_seconds",
             "Time spent in sync_reflect (audit + WAL + actor)",
             &self.sync_reflect_duration),
            ("cortex_approve_and_execute_duration_seconds",
             "Time spent in approve_and_execute (Kahn + dispatch)",
             &self.approve_and_execute_duration),
            ("cortex_recover_project_duration_seconds",
             "Time spent in recover_project (WAL replay)",
             &self.recover_project_duration),
            ("cortex_harvest_insights_duration_seconds",
             "Time spent in harvest_insights (InsightsHarvester LLM)",
             &self.harvest_insights_duration),
            ("cortex_llm_request_duration_seconds",
             "Time spent in raw LLM API calls (provider-agnostic)",
             &self.llm_request_duration),
        ];
        for (name, help, hist) in &histograms {
            out.push_str(&hist.snapshot().to_prometheus(name, help));
        }

        out
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Wrapper thread-safe pour partager les metrics entre tasks.
pub type SharedMetrics = Arc<Metrics>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

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

    // ============ Histogram tests (Session 6 option A) ============

    #[test]
    fn test_histogram_default_zero() {
        let h = Histogram::default();
        let snap = h.snapshot();
        assert_eq!(snap.count, 0);
        assert_eq!(snap.sum_micros, 0);
        for (_, c) in &snap.buckets {
            assert_eq!(*c, 0);
        }
    }

    #[test]
    fn test_histogram_observe_single_value() {
        let h = Histogram::default();
        h.observe(Duration::from_millis(50)); // 0.05s
        let snap = h.snapshot();
        assert_eq!(snap.count, 1);
        assert_eq!(snap.sum_micros, 50_000);
        // 0.05s : buckets > 0.05 sont à 0, buckets <= 0.05 sont à 1 (incluant +Inf)
        // 0.001, 0.0025, 0.005, 0.01, 0.025 → 0 (car 0.05 > ces valeurs)
        // 0.05, 0.1, 0.25, 0.5, 1, 2.5, 5, 10, 30, +Inf → 1 (car 0.05 <= ces valeurs)
        let expected_zero = [0.001, 0.0025, 0.005, 0.01, 0.025];
        for (le, c) in &snap.buckets {
            if le.is_infinite() {
                assert_eq!(*c, 1, "bucket +Inf = count total = 1");
            } else if expected_zero.contains(le) {
                assert_eq!(*c, 0, "bucket[le={}] should be 0 (0.05 > {})", le, le);
            } else {
                assert_eq!(*c, 1, "bucket[le={}] should be 1 (0.05 <= {})", le, le);
            }
        }
    }

    #[test]
    fn test_histogram_buckets_cumulative_invariant() {
        // Invariant Prometheus : bucket[i] >= bucket[i-1] pour buckets triés asc.
        let h = Histogram::default();
        for ms in [1, 5, 25, 100, 500, 2_000, 15_000] {
            h.observe(Duration::from_millis(ms));
        }
        let snap = h.snapshot();
        let mut prev = 0u64;
        for (le, c) in &snap.buckets {
            assert!(
                *c >= prev,
                "bucket[le={}] should be >= previous ({} >= {})",
                le, c, prev
            );
            prev = *c;
        }
        // Le dernier bucket (+Inf) = count total
        assert_eq!(snap.count, 7);
        assert_eq!(snap.buckets.last().unwrap().1, 7);
    }

    #[test]
    fn test_histogram_observe_large_value_goes_in_inf() {
        let h = Histogram::default();
        h.observe(Duration::from_secs(60)); // dépasse tous les buckets
        let snap = h.snapshot();
        assert_eq!(snap.count, 1);
        // 60s > 30s (max bucket) → seul +Inf doit être à 1
        for (le, c) in &snap.buckets {
            if le.is_infinite() {
                assert_eq!(*c, 1, "bucket +Inf = count total = 1");
            } else {
                assert_eq!(*c, 0, "bucket[le={}] should be 0 (60s > {})", le, le);
            }
        }
    }

    #[test]
    fn test_histogram_prometheus_format() {
        let h = Histogram::default();
        h.observe(Duration::from_millis(10));
        h.observe(Duration::from_millis(10));
        h.observe(Duration::from_secs(2));
        let snap = h.snapshot();
        let text = snap.to_prometheus("cortex_test_duration_seconds", "Test histogram");

        assert!(text.contains("# HELP cortex_test_duration_seconds Test histogram"));
        assert!(text.contains("# TYPE cortex_test_duration_seconds histogram"));
        assert!(text.contains("cortex_test_duration_seconds_bucket{le=\"0.001\"} 0"));
        assert!(text.contains("cortex_test_duration_seconds_bucket{le=\"+Inf\"} 3"));
        assert!(text.contains("cortex_test_duration_seconds_sum 2.02"));
        assert!(text.contains("cortex_test_duration_seconds_count 3"));
    }

    #[test]
    fn test_histogram_concurrent_observations() {
        let h = Arc::new(Histogram::default());
        let mut handles = vec![];
        for _ in 0..10 {
            let h2 = h.clone();
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    h2.observe(Duration::from_micros(500));
                }
            }));
        }
        for hh in handles {
            hh.join().unwrap();
        }
        let snap = h.snapshot();
        assert_eq!(snap.count, 1000);
        // 500µs = 0.0005s, en-dessous du plus petit bucket (0.001s)
        // donc AUCUN bucket ne devrait être incrémenté... sauf +Inf
        // Wait: 0.0005 <= 0.001? oui ! donc buckets[0.001] et tous les suivants = 1000
        for (le, c) in &snap.buckets {
            assert_eq!(*c, 1000, "bucket[le={}] should be 1000", le);
        }
    }

    #[test]
    fn test_histogram_timer_auto_observes_on_drop() {
        let h = Histogram::default();
        {
            let _timer = HistogramTimer::start(&h);
            std::thread::sleep(Duration::from_millis(10));
            // timer observe à la drop
        }
        let snap = h.snapshot();
        assert_eq!(snap.count, 1);
        assert!(snap.sum_micros >= 10_000);
    }

    #[test]
    fn test_metrics_prometheus_includes_histograms() {
        let m = Metrics::new();
        m.intercept_plan_duration.observe(Duration::from_millis(100));
        m.pre_mortem_duration.observe(Duration::from_millis(50));
        let out = m.to_prometheus();
        // Vérif qu'on retrouve les 8 histogrammes dans la sortie
        assert!(out.contains("cortex_intercept_plan_duration_seconds_bucket"));
        assert!(out.contains("cortex_pre_mortem_duration_seconds_bucket"));
        assert!(out.contains("cortex_red_team_audit_duration_seconds_bucket"));
        assert!(out.contains("cortex_sync_reflect_duration_seconds_bucket"));
        assert!(out.contains("cortex_approve_and_execute_duration_seconds_bucket"));
        assert!(out.contains("cortex_recover_project_duration_seconds_bucket"));
        assert!(out.contains("cortex_harvest_insights_duration_seconds_bucket"));
        assert!(out.contains("cortex_llm_request_duration_seconds_bucket"));
        // Et les counts
        assert!(out.contains("cortex_intercept_plan_duration_seconds_count 1"));
        assert!(out.contains("cortex_pre_mortem_duration_seconds_count 1"));
    }
}
