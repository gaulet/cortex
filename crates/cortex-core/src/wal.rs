//! Write-Ahead Log (WAL) service for crash recovery.
//!
//! WAL records every mutation to a scratchpad before applying it, so that
//! after a crash the state can be rebuilt from the WAL entries.
//!
//! Placeholder for now. Real SQLite-backed implementation comes later in Phase 4.

use crate::error::Result;

/// WAL service stub.
///
/// Will later be backed by SQLite with `sqlite/sqlx` + WAL mode enabled.
pub struct WalService {
    _placeholder: (),
}

impl WalService {
    pub fn new_placeholder() -> Result<Self> {
        Ok(Self { _placeholder: () })
    }

    /// Write a "prepare" entry (intention) to the WAL.
    pub async fn write_prepare(&self, _project_id: &str, _action: &str) -> Result<()> {
        Ok(())
    }

    /// Commit a prepared entry (confirmation the action happened).
    pub async fn write_commit(&self) -> Result<()> {
        Ok(())
    }
}
