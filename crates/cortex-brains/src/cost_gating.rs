//! Cost-gating logic (§6.5 specification-cortex-v2.md).
//!
//! Determines which LLM brains to activate for a given job, based on
//! the job's criticity score. This is how Cortex controls token spend.

/// Static cost-gating helper.
pub struct CostGating;

impl CostGating {
    /// Pre-Mortem is activated for criticity >= 4.
    pub fn should_run_pre_mortem(criticity: u8) -> bool {
        criticity >= 4
    }

    /// Red-Team is activated for criticity >= 3.
    pub fn should_run_red_team(criticity: u8) -> bool {
        criticity >= 3
    }

    /// HMAC signature required for criticity >= 3.
    pub fn should_require_hmac(criticity: u8) -> bool {
        criticity >= 3
    }

    /// Estimated total tokens per job based on criticity.
    pub fn estimate_tokens(criticity: u8) -> u32 {
        let pre_mortem = if Self::should_run_pre_mortem(criticity) { 1500 } else { 0 };
        let red_team = if Self::should_run_red_team(criticity) { 2500 } else { 0 };
        pre_mortem + red_team
    }
}

/// Decision struct returned by `activate_brains_for_job`.
#[derive(Debug, Clone)]
pub struct BrainActivation {
    pub architect: bool,
    pub pre_mortem: bool,
    pub red_team: bool,
    pub hmac_required: bool,
    pub estimated_tokens: u32,
}

/// Decide which brains to activate for a given criticity.
///
/// The Architect is NOT activated here - it's always called on intercept_plan,
/// before any job is created. This function is for PER-JOB decisions.
pub fn activate_brains_for_job(criticity: u8) -> BrainActivation {
    BrainActivation {
        architect: false,
        pre_mortem: CostGating::should_run_pre_mortem(criticity),
        red_team: CostGating::should_run_red_team(criticity),
        hmac_required: CostGating::should_require_hmac(criticity),
        estimated_tokens: CostGating::estimate_tokens(criticity),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cost_gating_criticity_1() {
        let activation = activate_brains_for_job(1);
        assert!(!activation.pre_mortem);
        assert!(!activation.red_team);
        assert!(!activation.hmac_required);
        assert_eq!(activation.estimated_tokens, 0);
    }

    #[test]
    fn test_cost_gating_criticity_3() {
        let activation = activate_brains_for_job(3);
        assert!(!activation.pre_mortem);
        assert!(activation.red_team);
        assert!(activation.hmac_required);
        assert_eq!(activation.estimated_tokens, 2500);
    }

    #[test]
    fn test_cost_gating_criticity_5() {
        let activation = activate_brains_for_job(5);
        assert!(activation.pre_mortem);
        assert!(activation.red_team);
        assert!(activation.hmac_required);
        assert_eq!(activation.estimated_tokens, 4000);
    }
}
