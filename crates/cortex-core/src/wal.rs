//! Write-Ahead Log (WAL) service for crash recovery.
//!
//! # Architecture
//!
//! WAL enregistre chaque mutation à un scratchpad AVANT application, permettant
//! reconstruction après crash. Deux mécanismes de persistance :
//!
//! 1. **WAL Entries** : Journal détaillé des mutations (prepare + commit)
//! 2. **Cortex Commits** : Snapshots complets d'état projet (diff + full snapshot)
//! 3. **Cortex States** : État courant (cache rapide pour lecture)
//!
//! # Pattern d'utilisation
//!
//! ```rust,ignore
//! let wal = WalService::connect("sqlite:cortex.db").await?;
//! let entry_id = wal.write_prepare("proj_1", "job_added", None, &json!({..})).await?;
//! // ... application mutation au Scratchpad ...
//! wal.write_commit(&entry_id).await?;
//! ```
//!
//! # Crash Recovery
//!
//! Au démarrage, Cortex scanne :
//! - WAL entries non committés → rollback des mutations en vol
//! - Snapshot dans cortex_states → état courant
//! - Cortex commits → historique pour rollback user

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use sqlx::Row;
use thiserror::Error;
use uuid::Uuid;

use crate::error::{CortexError, Result};

/// Erreurs spécifiques au WAL service.
#[derive(Error, Debug)]
pub enum WalError {
    #[error("Database connection failed: {0}")]
    ConnectionFailed(String),
    #[error("Migration failed: {0}")]
    MigrationFailed(String),
    #[error("WAL entry not found: {0}")]
    EntryNotFound(String),
    #[error("Entry already committed: {0}")]
    AlreadyCommitted(String),
    #[error("Corruption detected: {0}")]
    Corruption(String),
}

impl From<WalError> for CortexError {
    fn from(err: WalError) -> Self {
        CortexError::WalError(err.to_string())
    }
}

impl From<sqlx::Error> for WalError {
    fn from(err: sqlx::Error) -> Self {
        WalError::ConnectionFailed(err.to_string())
    }
}

/// Entry WAL représentant une mutation en préparation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalEntry {
    pub entry_id: String,
    pub project_id: String,
    pub action: String,
    pub job_id: Option<String>,
    pub theme_id: Option<String>,
    pub data: JsonValue,
    pub committed: bool,
    pub created_at: i64,
}

/// Commit représentant un snapshot d'état projet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CortexCommit {
    pub commit_id: String,
    pub project_id: String,
    pub timestamp: i64,
    pub previous_commit: Option<String>,
    pub mutation_type: String,
    pub trigger_reason: String,
    pub diff: JsonValue,
    pub snapshot: JsonValue,
    pub checksum: String,
}

/// Service WAL pour gestion persistance et crash recovery.
#[derive(Clone)]
pub struct WalService {
    pool: SqlitePool,
}

impl WalService {
    /// Connecte au service WAL avec migrations automatiques.
    ///
    /// `url` peut être :
    /// - `"sqlite:./cortex.db"` (fichier)
    /// - `"sqlite::memory:"` (tests)
    pub async fn connect(url: &str) -> Result<Self> {
        let opts: SqliteConnectOptions = url
            .parse()
            .map_err(|e: sqlx::Error| CortexError::WalError(format!("invalid URL: {}", e)))?;

        // Activer WAL mode dans SQLite pour performance
        let opts = opts
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .synchronous(sqlx::sqlite::SqliteSynchronous::Normal)
            .busy_timeout(std::time::Duration::from_secs(5));

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(opts)
            .await
            .map_err(|e| CortexError::WalError(format!("pool connection failed: {}", e)))?;

        let service = Self { pool };
        service.run_migrations().await?;

        Ok(service)
    }

    /// Exécute les migrations SQL depuis le dossier migrations/.
    async fn run_migrations(&self) -> Result<()> {
        // Migration inline pour éviter dépendance runtime au dossier migrations
        // TODO : migrer vers sqlx::migrate!("migrations") une fois testé
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS wal_entries (
                entry_id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                action TEXT NOT NULL,
                job_id TEXT,
                theme_id TEXT,
                data_json TEXT NOT NULL,
                committed BOOLEAN NOT NULL DEFAULT 0,
                created_at INTEGER NOT NULL DEFAULT (unixepoch())
            );
            CREATE INDEX IF NOT EXISTS idx_wal_project_id ON wal_entries(project_id);
            CREATE INDEX IF NOT EXISTS idx_wal_uncommitted ON wal_entries(project_id, committed);
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| CortexError::WalError(format!("migration wal_entries failed: {}", e)))?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS cortex_commits (
                commit_id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                previous_commit TEXT,
                mutation_type TEXT NOT NULL,
                trigger_reason TEXT NOT NULL,
                diff_json TEXT NOT NULL,
                snapshot_json TEXT NOT NULL,
                checksum TEXT NOT NULL,
                created_at INTEGER NOT NULL DEFAULT (unixepoch())
            );
            CREATE INDEX IF NOT EXISTS idx_commits_project ON cortex_commits(project_id, timestamp);
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| CortexError::WalError(format!("migration cortex_commits failed: {}", e)))?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS cortex_states (
                project_id TEXT PRIMARY KEY,
                state_json TEXT NOT NULL,
                last_updated INTEGER NOT NULL,
                handoff_summary TEXT
            );
            "#,
        )
        .execute(&self.pool)
        .await
        .map_err(|e| CortexError::WalError(format!("migration cortex_states failed: {}", e)))?;

        Ok(())
    }

    /// Écrit une entrée WAL préparée (mutation en attente).
    ///
    /// Retourne entry_id pour pouvoir committer ensuite.
    pub async fn write_prepare(
        &self,
        project_id: &str,
        action: &str,
        job_id: Option<&str>,
        theme_id: Option<&str>,
        data: &JsonValue,
    ) -> Result<String> {
        let entry_id = Uuid::now_v7().to_string();
        let timestamp = Utc::now().timestamp_millis();
        let data_json = serde_json::to_string(data)?;

        sqlx::query(
            r#"
            INSERT INTO wal_entries (entry_id, project_id, timestamp, action, job_id, theme_id, data_json, committed)
            VALUES (?, ?, ?, ?, ?, ?, ?, 0)
            "#,
        )
        .bind(&entry_id)
        .bind(project_id)
        .bind(timestamp)
        .bind(action)
        .bind(job_id)
        .bind(theme_id)
        .bind(&data_json)
        .execute(&self.pool)
        .await
        .map_err(|e| CortexError::WalError(format!("write_prepare failed: {}", e)))?;

        Ok(entry_id)
    }

    /// Commit une entrée WAL préparée.
    pub async fn write_commit(&self, entry_id: &str) -> Result<()> {
        let result = sqlx::query(
            "UPDATE wal_entries SET committed = 1 WHERE entry_id = ? AND committed = 0",
        )
        .bind(entry_id)
        .execute(&self.pool)
        .await
        .map_err(|e| CortexError::WalError(format!("write_commit failed: {}", e)))?;

        if result.rows_affected() == 0 {
            return Err(CortexError::WalError(format!(
                "Entry {} not found or already committed",
                entry_id
            )));
        }

        Ok(())
    }

    /// Lit une entrée WAL par son ID.
    pub async fn read_entry(&self, entry_id: &str) -> Result<WalEntry> {
        let row = sqlx::query(
            r#"
            SELECT entry_id, project_id, timestamp, action, job_id, theme_id, data_json, committed, created_at
            FROM wal_entries WHERE entry_id = ?
            "#,
        )
        .bind(entry_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| CortexError::WalError(format!("read_entry failed: {}", e)))?;

        let data_json: String = row.get("data_json");
        let data: JsonValue = serde_json::from_str(&data_json)
            .map_err(|e| CortexError::WalError(format!("JSON parse error: {}", e)))?;

        Ok(WalEntry {
            entry_id: row.get("entry_id"),
            project_id: row.get("project_id"),
            action: row.get("action"),
            job_id: row.get("job_id"),
            theme_id: row.get("theme_id"),
            data,
            committed: row.get("committed"),
            created_at: row.get("created_at"),
        })
    }

    /// Liste les entrées WAL non committées pour un projet (pour crash recovery).
    pub async fn list_uncommitted(&self, project_id: &str) -> Result<Vec<WalEntry>> {
        let rows = sqlx::query(
            r#"
            SELECT entry_id, project_id, timestamp, action, job_id, theme_id, data_json, committed, created_at
            FROM wal_entries WHERE project_id = ? AND committed = 0
            ORDER BY timestamp ASC
            "#,
        )
        .bind(project_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| CortexError::WalError(format!("list_uncommitted failed: {}", e)))?;

        rows.into_iter()
            .map(|row| {
                let data_json: String = row.get("data_json");
                let data: JsonValue = serde_json::from_str(&data_json)
                    .map_err(|e| CortexError::WalError(format!("JSON parse error: {}", e)))?;

                Ok(WalEntry {
                    entry_id: row.get("entry_id"),
                    project_id: row.get("project_id"),
                    action: row.get("action"),
                    job_id: row.get("job_id"),
                    theme_id: row.get("theme_id"),
                    data,
                    committed: false,
                    created_at: row.get("created_at"),
                })
            })
            .collect()
    }

    /// Rollback de toutes les entrées WAL non committées d'un projet (après crash).
    pub async fn rollback_uncommitted(&self, project_id: &str) -> Result<usize> {
        let result = sqlx::query("DELETE FROM wal_entries WHERE project_id = ? AND committed = 0")
            .bind(project_id)
            .execute(&self.pool)
            .await
            .map_err(|e| CortexError::WalError(format!("rollback failed: {}", e)))?;

        Ok(result.rows_affected() as usize)
    }

    /// Écrit un commit (snapshot complet d'état projet).
    pub async fn write_snapshot_commit(
        &self,
        project_id: &str,
        mutation_type: &str,
        trigger_reason: &str,
        diff: &JsonValue,
        snapshot: &JsonValue,
    ) -> Result<String> {
        let commit_id = format!("commit_{}", Uuid::now_v7());
        let timestamp = Utc::now().timestamp_millis();

        // Récupère le dernier commit du projet
        let previous_commit: Option<String> = sqlx::query_scalar(
            "SELECT commit_id FROM cortex_commits WHERE project_id = ? ORDER BY timestamp DESC LIMIT 1",
        )
        .bind(project_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| CortexError::WalError(format!("fetch previous commit failed: {}", e)))?;

        let diff_json = serde_json::to_string(diff)?;
        let snapshot_json = serde_json::to_string(snapshot)?;

        // Checksum = SHA256(commit_id + timestamp + previous + snapshot_json)
        let checksum_input = format!(
            "{}:{}:{:?}:{}",
            commit_id, timestamp, previous_commit, snapshot_json
        );
        let checksum = sha256_hex(&checksum_input);

        sqlx::query(
            r#"
            INSERT INTO cortex_commits
            (commit_id, project_id, timestamp, previous_commit, mutation_type, trigger_reason, diff_json, snapshot_json, checksum)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&commit_id)
        .bind(project_id)
        .bind(timestamp)
        .bind(&previous_commit)
        .bind(mutation_type)
        .bind(trigger_reason)
        .bind(&diff_json)
        .bind(&snapshot_json)
        .bind(&checksum)
        .execute(&self.pool)
        .await
        .map_err(|e| CortexError::WalError(format!("write snapshot commit failed: {}", e)))?;

        Ok(commit_id)
    }

    /// Liste les commits d'un projet, du plus récent au plus ancien.
    pub async fn list_commits(&self, project_id: &str) -> Result<Vec<CortexCommit>> {
        let rows = sqlx::query(
            r#"
            SELECT commit_id, project_id, timestamp, previous_commit, mutation_type, trigger_reason, diff_json, snapshot_json, checksum
            FROM cortex_commits WHERE project_id = ?
            ORDER BY timestamp DESC
            "#,
        )
        .bind(project_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| CortexError::WalError(format!("list commits failed: {}", e)))?;

        rows.into_iter()
            .map(|row| {
                let diff_json: String = row.get("diff_json");
                let snapshot_json: String = row.get("snapshot_json");
                Ok(CortexCommit {
                    commit_id: row.get("commit_id"),
                    project_id: row.get("project_id"),
                    timestamp: row.get("timestamp"),
                    previous_commit: row.get("previous_commit"),
                    mutation_type: row.get("mutation_type"),
                    trigger_reason: row.get("trigger_reason"),
                    diff: serde_json::from_str(&diff_json)
                        .map_err(|e| CortexError::WalError(format!("diff parse error: {}", e)))?,
                    snapshot: serde_json::from_str(&snapshot_json)
                        .map_err(|e| CortexError::WalError(format!("snapshot parse error: {}", e)))?,
                    checksum: row.get("checksum"),
                })
            })
            .collect()
    }

    /// Charge le dernier commit d'un projet.
    pub async fn latest_commit(&self, project_id: &str) -> Result<Option<CortexCommit>> {
        let commits = self.list_commits(project_id).await?;
        Ok(commits.into_iter().next())
    }

    /// Sauvegarde l'état courant d'un projet (cache rapide).
    pub async fn save_state(
        &self,
        project_id: &str,
        state: &JsonValue,
        handoff_summary: Option<&str>,
    ) -> Result<()> {
        let state_json = serde_json::to_string(state)?;
        let timestamp = Utc::now().timestamp_millis();

        sqlx::query(
            r#"
            INSERT INTO cortex_states (project_id, state_json, last_updated, handoff_summary)
            VALUES (?, ?, ?, ?)
            ON CONFLICT(project_id) DO UPDATE SET
                state_json = excluded.state_json,
                last_updated = excluded.last_updated,
                handoff_summary = excluded.handoff_summary
            "#,
        )
        .bind(project_id)
        .bind(&state_json)
        .bind(timestamp)
        .bind(handoff_summary)
        .execute(&self.pool)
        .await
        .map_err(|e| CortexError::WalError(format!("save state failed: {}", e)))?;

        Ok(())
    }

    /// Charge l'état courant d'un projet.
    pub async fn load_state(&self, project_id: &str) -> Result<Option<JsonValue>> {
        let row = sqlx::query("SELECT state_json FROM cortex_states WHERE project_id = ?")
            .bind(project_id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| CortexError::WalError(format!("load state failed: {}", e)))?;

        match row {
            Some(row) => {
                let state_json: String = row.get("state_json");
                let state: JsonValue = serde_json::from_str(&state_json)?;
                Ok(Some(state))
            }
            None => Ok(None),
        }
    }

    /// Référence interne vers le pool (pour usage avancé, ex: dans cortex-actors).
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }
}

/// Helper SHA256 simple (sans dépendance externe)
fn sha256_hex(input: &str) -> String {
    // Utilise sha2 crate via cortex-security si disponible, sinon fallback simple
    // Pour l'instant implémentation simple avec hash de string
    // TODO : remplacer par vrai SHA256 via cortex-security
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    input.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    async fn test_wal() -> WalService {
        WalService::connect("sqlite::memory:")
            .await
            .expect("in-memory DB should connect")
    }

    #[tokio::test]
    async fn test_connect_in_memory() {
        let wal = test_wal().await;
        assert!(wal.pool().size() > 0);
    }

    #[tokio::test]
    async fn test_write_prepare_and_commit() {
        let wal = test_wal().await;

        let entry_id = wal
            .write_prepare(
                "proj_1",
                "theme_added",
                None,
                Some("TH-1"),
                &json!({"name": "Audit"}),
            )
            .await
            .expect("prepare should succeed");

        let entry = wal.read_entry(&entry_id).await.expect("should read");
        assert_eq!(entry.action, "theme_added");
        assert!(!entry.committed);

        wal.write_commit(&entry_id).await.expect("commit should succeed");

        let entry = wal.read_entry(&entry_id).await.expect("should read");
        assert!(entry.committed);
    }

    #[tokio::test]
    async fn test_double_commit_fails() {
        let wal = test_wal().await;

        let entry_id = wal
            .write_prepare("proj_1", "action", None, None, &json!({}))
            .await
            .unwrap();
        wal.write_commit(&entry_id).await.unwrap();

        let result = wal.write_commit(&entry_id).await;
        assert!(result.is_err(), "double commit should fail");
    }

    #[tokio::test]
    async fn test_list_uncommitted() {
        let wal = test_wal().await;

        wal.write_prepare("proj_1", "action_1", None, None, &json!({"i": 1}))
            .await
            .unwrap();
        wal.write_prepare("proj_1", "action_2", None, None, &json!({"i": 2}))
            .await
            .unwrap();
        let committed_id = wal
            .write_prepare("proj_1", "action_3", None, None, &json!({"i": 3}))
            .await
            .unwrap();
        wal.write_commit(&committed_id).await.unwrap();

        let uncommitted = wal.list_uncommitted("proj_1").await.unwrap();
        assert_eq!(uncommitted.len(), 2);
        assert_eq!(uncommitted[0].action, "action_1");
        assert_eq!(uncommitted[1].action, "action_2");
    }

    #[tokio::test]
    async fn test_rollback_uncommitted() {
        let wal = test_wal().await;

        wal.write_prepare("proj_1", "action_1", None, None, &json!({}))
            .await
            .unwrap();
        wal.write_prepare("proj_1", "action_2", None, None, &json!({}))
            .await
            .unwrap();

        let deleted = wal.rollback_uncommitted("proj_1").await.unwrap();
        assert_eq!(deleted, 2);

        let uncommitted = wal.list_uncommitted("proj_1").await.unwrap();
        assert_eq!(uncommitted.len(), 0);
    }

    #[tokio::test]
    async fn test_write_and_read_snapshot_commit() {
        let wal = test_wal().await;

        let commit_id = wal
            .write_snapshot_commit(
                "proj_1",
                "project_created",
                "User requested refactor",
                &json!({"added": []}),
                &json!({"objective": "Refactor auth"}),
            )
            .await
            .expect("write snapshot should succeed");

        assert!(commit_id.starts_with("commit_"));

        let latest = wal
            .latest_commit("proj_1")
            .await
            .expect("latest should succeed");
        assert!(latest.is_some());
        let commit = latest.unwrap();
        assert_eq!(commit.commit_id, commit_id);
        assert_eq!(commit.mutation_type, "project_created");
    }

    #[tokio::test]
    async fn test_snapshot_chain_previous_commit() {
        let wal = test_wal().await;

        let c1 = wal
            .write_snapshot_commit(
                "proj_1",
                "initial",
                "r1",
                &json!({}),
                &json!({"s": 1}),
            )
            .await
            .unwrap();

        let c2 = wal
            .write_snapshot_commit(
                "proj_1",
                "mutation",
                "r2",
                &json!({"changed": true}),
                &json!({"s": 2}),
            )
            .await
            .unwrap();

        let commits = wal.list_commits("proj_1").await.unwrap();
        assert_eq!(commits.len(), 2);
        // Plus récent en premier
        assert_eq!(commits[0].commit_id, c2);
        assert_eq!(commits[0].previous_commit, Some(c1.clone()));
        assert_eq!(commits[1].commit_id, c1);
        assert_eq!(commits[1].previous_commit, None);
    }

    #[tokio::test]
    async fn test_save_and_load_state() {
        let wal = test_wal().await;

        wal.save_state(
            "proj_1",
            &json!({"objective": "Refactor auth", "themes": 3}),
            Some("3 themes planned"),
        )
        .await
        .expect("save state should succeed");

        let state = wal
            .load_state("proj_1")
            .await
            .expect("load state should succeed");
        assert!(state.is_some());
        assert_eq!(state.unwrap()["objective"], "Refactor auth");
    }

    #[tokio::test]
    async fn test_save_state_upsert() {
        let wal = test_wal().await;

        wal.save_state("proj_1", &json!({"v": 1}), None).await.unwrap();
        wal.save_state("proj_1", &json!({"v": 2}), Some("updated"))
            .await
            .unwrap();

        let state = wal.load_state("proj_1").await.unwrap().unwrap();
        assert_eq!(state["v"], 2);
    }

    #[tokio::test]
    async fn test_load_state_not_found() {
        let wal = test_wal().await;
        let state = wal.load_state("nonexistent").await.unwrap();
        assert!(state.is_none());
    }
}
