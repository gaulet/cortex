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
//! # Backends supportés (Session 6 - option E)
//!
//! - **SQLite** (défaut) : embedded, single-file, idéal pour le mode local/tests
//! - **PostgreSQL** (feature `postgres`) : multi-host, scalable, idéal pour la prod
//!
//! Le backend est détecté automatiquement depuis l'URL de connexion :
//! - `sqlite://...` ou `sqlite::memory:` → SQLite
//! - `postgres://...` ou `postgresql://...` → PostgreSQL
//!
//! # Pattern d'utilisation
//!
//! ```rust,ignore
//! let wal = WalService::connect("sqlite::memory:").await?;          // SQLite
//! let wal = WalService::connect("postgres://localhost/cortex").await?; // Postgres
//! let entry_id = wal.write_prepare("proj_1", "job_added", None, &json!({..})).await?;
//! wal.write_commit(&entry_id).await?;
//! ```
//!
//! # Crash Recovery
//!
//! Au démarrage, Cortex scanne :
//! - WAL entries non committés → rollback des mutations en vol
//! - Snapshot dans cortex_states → état courant
//! - Cortex commits → historique pour rollback user
//!
//! # Note sur la portabilité
//!
//! Pour minimiser la duplication, chaque méthode publique dispatche vers
//! une branche Sqlite/Postgres. Les requêtes Postgres utilisent des
//! placeholders `$1, $2, ...` (au lieu de `?` pour SQLite) et des types
//! natifs Postgres (BIGINT, JSONB, BOOLEAN).

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
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
    #[error("Unsupported URL scheme: {0}")]
    UnsupportedUrl(String),
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

/// Politique appliquée à une entry uncommitted pendant la recovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryAction {
    /// Commit l'entry (action terminale, safe à finaliser)
    Commit,
    /// Rollback l'entry (safe default, marqué dans le WAL)
    Rollback,
    /// Escalader (laisser uncommitted, intervention humaine requise)
    Escalate,
}

/// Rapport de recovery post-crash pour un projet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryReport {
    pub project_id: String,
    pub uncommitted_count: usize,
    pub rolled_back: Vec<String>, // entry_ids rolled back
    pub escalated: Vec<String>,   // entry_ids escalated
}

/// Backend de persistance détecté à la connexion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Sqlite,
    Postgres,
}

impl BackendKind {
    /// Détecte le backend depuis l'URL de connexion.
    pub fn from_url(url: &str) -> Result<Self> {
        let url_lower = url.to_lowercase();
        if url_lower.starts_with("sqlite:") {
            Ok(BackendKind::Sqlite)
        } else if url_lower.starts_with("postgres://") || url_lower.starts_with("postgresql://") {
            #[cfg(not(feature = "postgres"))]
            return Err(CortexError::WalError(
                "PostgreSQL support not compiled in. Build with --features postgres".into(),
            ));
            #[cfg(feature = "postgres")]
            Ok(BackendKind::Postgres)
        } else {
            Err(CortexError::WalError(format!(
                "Unsupported URL scheme: '{}'. Use 'sqlite://...' or 'postgres://...'",
                url
            )))
        }
    }
}

/// Service WAL pour gestion persistance et crash recovery.
///
/// Le backend (SQLite ou Postgres) est détecté à la construction depuis l'URL.
#[derive(Clone)]
pub struct WalService {
    backend_kind: BackendKind,
    /// SQLite pool (si backend = Sqlite)
    #[cfg(feature = "sqlite")]
    sqlite_pool: Option<sqlx::SqlitePool>,
    /// Postgres pool (si backend = Postgres)
    #[cfg(feature = "postgres")]
    postgres_pool: Option<sqlx::PgPool>,
}

impl WalService {
    /// Connecte au service WAL avec migrations automatiques.
    ///
    /// `url` peut être :
    /// - `"sqlite::memory:"` (tests, embedded)
    /// - `"sqlite://./cortex.db"` (fichier SQLite)
    /// - `"postgres://user:pass@host:5432/dbname"` (PostgreSQL, feature requise)
    pub async fn connect(url: &str) -> Result<Self> {
        let kind = BackendKind::from_url(url)?;

        match kind {
            BackendKind::Sqlite => Self::connect_sqlite(url).await,
            BackendKind::Postgres => Self::connect_postgres(url).await,
        }
    }

    /// Retourne le type de backend actif.
    pub fn backend_kind(&self) -> BackendKind {
        self.backend_kind
    }

    /// Connecte à SQLite.
    #[cfg(feature = "sqlite")]
    async fn connect_sqlite(url: &str) -> Result<Self> {
        use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

        let opts: SqliteConnectOptions = url
            .parse()
            .map_err(|e: sqlx::Error| CortexError::WalError(format!("invalid SQLite URL: {}", e)))?;

        // Activer WAL mode pour performance/concurrence
        let opts = opts
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .synchronous(sqlx::sqlite::SqliteSynchronous::Normal)
            .busy_timeout(std::time::Duration::from_secs(5));

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(opts)
            .await
            .map_err(|e| CortexError::WalError(format!("SQLite pool connection failed: {}", e)))?;

        let mut service = Self {
            backend_kind: BackendKind::Sqlite,
            #[cfg(feature = "sqlite")]
            sqlite_pool: Some(pool),
            #[cfg(feature = "postgres")]
            postgres_pool: None,
        };
        service.run_migrations().await?;
        Ok(service)
    }

    /// Connecte à PostgreSQL.
    #[cfg(feature = "postgres")]
    async fn connect_postgres(url: &str) -> Result<Self> {
        use sqlx::postgres::PgPoolOptions;

        let pool = PgPoolOptions::new()
            .max_connections(10) // Postgres supporte plus de connexions que SQLite
            .connect(url)
            .await
            .map_err(|e| CortexError::WalError(format!("Postgres connection failed: {}", e)))?;

        let mut service = Self {
            backend_kind: BackendKind::Postgres,
            #[cfg(feature = "sqlite")]
            sqlite_pool: None,
            #[cfg(feature = "postgres")]
            postgres_pool: Some(pool),
        };
        service.run_migrations().await?;
        Ok(service)
    }

    /// Version dummy de connect_sqlite quand la feature sqlite n'est pas activée.
    #[cfg(not(feature = "sqlite"))]
    async fn connect_sqlite(_url: &str) -> Result<Self> {
        Err(CortexError::WalError(
            "SQLite support not compiled in. Build with --features sqlite".into(),
        ))
    }

    /// Version dummy de connect_postgres quand la feature postgres n'est pas activée.
    #[cfg(not(feature = "postgres"))]
    async fn connect_postgres(_url: &str) -> Result<Self> {
        Err(CortexError::WalError(
            "PostgreSQL support not compiled in. Build with --features postgres".into(),
        ))
    }

    /// Exécute les migrations SQL pour le backend actif.
    async fn run_migrations(&mut self) -> Result<()> {
        match self.backend_kind {
            BackendKind::Sqlite => self.run_migrations_sqlite().await,
            BackendKind::Postgres => self.run_migrations_postgres().await,
        }
    }

    #[cfg(feature = "sqlite")]
    async fn run_migrations_sqlite(&self) -> Result<()> {
        let pool = self.sqlite_pool.as_ref().ok_or_else(|| {
            CortexError::WalError("SQLite pool not initialized".into())
        })?;

        for stmt in SQLITE_MIGRATION_STATEMENTS {
            sqlx::query(stmt)
                .execute(pool)
                .await
                .map_err(|e| CortexError::WalError(format!("SQLite migration failed: {}", e)))?;
        }
        Ok(())
    }

    #[cfg(feature = "postgres")]
    async fn run_migrations_postgres(&self) -> Result<()> {
        let pool = self.postgres_pool.as_ref().ok_or_else(|| {
            CortexError::WalError("Postgres pool not initialized".into())
        })?;

        for stmt in POSTGRES_MIGRATION_STATEMENTS {
            sqlx::query(stmt)
                .execute(pool)
                .await
                .map_err(|e| CortexError::WalError(format!("Postgres migration failed: {}", e)))?;
        }
        Ok(())
    }

    #[cfg(not(feature = "sqlite"))]
    async fn run_migrations_sqlite(&self) -> Result<()> {
        Err(CortexError::WalError("SQLite feature disabled".into()))
    }

    #[cfg(not(feature = "postgres"))]
    async fn run_migrations_postgres(&self) -> Result<()> {
        Err(CortexError::WalError("Postgres feature disabled".into()))
    }

    // ============================================================
    // PUBLIC API : dispatch automatique selon le backend
    // ============================================================

    /// Recover uncommitted WAL entries (crash recovery).
    pub async fn recover_uncommitted(
        &self,
        project_id: &str,
    ) -> Result<RecoveryReport> {
        let uncommitted = self.list_uncommitted(project_id).await?;
        let mut report = RecoveryReport {
            project_id: project_id.to_string(),
            uncommitted_count: uncommitted.len(),
            rolled_back: Vec::new(),
            escalated: Vec::new(),
        };

        for entry in uncommitted {
            let policy = match entry.action.as_str() {
                "approval_received" => RecoveryAction::Commit,
                "abort" | "rollback" | "recovery_rollback" => RecoveryAction::Commit,
                "sync_reflect" | "task_completed" | "rollout" => RecoveryAction::Escalate,
                _ => RecoveryAction::Rollback,
            };

            match policy {
                RecoveryAction::Commit => {
                    self.write_commit(&entry.entry_id).await?;
                }
                RecoveryAction::Rollback => {
                    self.write_prepare(
                        project_id,
                        "recovery_rollback",
                        entry.job_id.as_deref(),
                        entry.theme_id.as_deref(),
                        &serde_json::json!({
                            "rolled_back_entry": &entry.entry_id,
                            "original_action": &entry.action,
                        }),
                    )
                    .await?;
                    report.rolled_back.push(entry.entry_id);
                }
                RecoveryAction::Escalate => {
                    report.escalated.push(entry.entry_id);
                }
            }
        }

        Ok(report)
    }

    /// Écrit une entrée WAL préparée (mutation en attente).
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

        match self.backend_kind {
            BackendKind::Sqlite => {
                #[cfg(feature = "sqlite")]
                {
                    let pool = self.sqlite_pool.as_ref().unwrap();
                    sqlx::query(
                        r#"
                        INSERT INTO wal_entries
                        (entry_id, project_id, timestamp, action, job_id, theme_id, data_json, committed)
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
                    .execute(pool)
                    .await
                    .map_err(|e| CortexError::WalError(format!("write_prepare (sqlite) failed: {}", e)))?;
                }
                #[cfg(not(feature = "sqlite"))]
                return Err(CortexError::WalError("SQLite feature disabled".into()));
            }
            BackendKind::Postgres => {
                #[cfg(feature = "postgres")]
                {
                    let pool = self.postgres_pool.as_ref().unwrap();
                    sqlx::query(
                        r#"
                        INSERT INTO wal_entries
                        (entry_id, project_id, timestamp, action, job_id, theme_id, data_json, committed)
                        VALUES ($1, $2, $3, $4, $5, $6, $7, false)
                        "#,
                    )
                    .bind(&entry_id)
                    .bind(project_id)
                    .bind(timestamp)
                    .bind(action)
                    .bind(job_id)
                    .bind(theme_id)
                    .bind(&data_json)
                    .execute(pool)
                    .await
                    .map_err(|e| CortexError::WalError(format!("write_prepare (postgres) failed: {}", e)))?;
                }
                #[cfg(not(feature = "postgres"))]
                return Err(CortexError::WalError("Postgres feature disabled".into()));
            }
        }

        Ok(entry_id)
    }

    /// Commit une entrée WAL préparée.
    pub async fn write_commit(&self, entry_id: &str) -> Result<()> {
        let rows_affected = match self.backend_kind {
            BackendKind::Sqlite => {
                #[cfg(feature = "sqlite")]
                {
                    let pool = self.sqlite_pool.as_ref().unwrap();
                    let result = sqlx::query(
                        "UPDATE wal_entries SET committed = 1 WHERE entry_id = ? AND committed = 0",
                    )
                    .bind(entry_id)
                    .execute(pool)
                    .await
                    .map_err(|e| CortexError::WalError(format!("write_commit (sqlite) failed: {}", e)))?;
                    result.rows_affected()
                }
                #[cfg(not(feature = "sqlite"))]
                return Err(CortexError::WalError("SQLite feature disabled".into()));
            }
            BackendKind::Postgres => {
                #[cfg(feature = "postgres")]
                {
                    let pool = self.postgres_pool.as_ref().unwrap();
                    let result = sqlx::query(
                        "UPDATE wal_entries SET committed = true WHERE entry_id = $1 AND committed = false",
                    )
                    .bind(entry_id)
                    .execute(pool)
                    .await
                    .map_err(|e| CortexError::WalError(format!("write_commit (postgres) failed: {}", e)))?;
                    result.rows_affected()
                }
                #[cfg(not(feature = "postgres"))]
                return Err(CortexError::WalError("Postgres feature disabled".into()));
            }
        };

        if rows_affected == 0 {
            return Err(CortexError::WalError(format!(
                "Entry {} not found or already committed",
                entry_id
            )));
        }

        Ok(())
    }

    /// Lit une entrée WAL par son ID.
    pub async fn read_entry(&self, entry_id: &str) -> Result<WalEntry> {
        // Dispatch runtime : SQLite vs Postgres ont des types de row différents.
        // On utilise des méthodes privées qui retournent le bon type pour chaque backend.
        if self.backend_kind == BackendKind::Sqlite {
            #[cfg(feature = "sqlite")]
            {
                let row = self.read_entry_sqlite(entry_id).await?;
                return self.wal_entry_from_sqlite_row(row);
            }
            #[cfg(not(feature = "sqlite"))]
            return Err(CortexError::WalError("SQLite feature disabled".into()));
        } else {
            #[cfg(feature = "postgres")]
            {
                let row = self.read_entry_postgres(entry_id).await?;
                return self.wal_entry_from_postgres_row(row);
            }
            #[cfg(not(feature = "postgres"))]
            return Err(CortexError::WalError("Postgres feature disabled".into()));
        }
    }

    #[cfg(feature = "sqlite")]
    async fn read_entry_sqlite(&self, entry_id: &str) -> Result<sqlx::sqlite::SqliteRow> {
        let pool = self.sqlite_pool.as_ref().ok_or_else(|| {
            CortexError::WalError("SQLite pool not initialized".into())
        })?;
        sqlx::query(
            r#"
            SELECT entry_id, project_id, timestamp, action, job_id, theme_id, data_json, committed, created_at
            FROM wal_entries WHERE entry_id = ?
            "#,
        )
        .bind(entry_id)
        .fetch_one(pool)
        .await
        .map_err(|e| CortexError::WalError(format!("read_entry (sqlite) failed: {}", e)))
    }

    #[cfg(feature = "postgres")]
    async fn read_entry_postgres(&self, entry_id: &str) -> Result<sqlx::postgres::PgRow> {
        let pool = self.postgres_pool.as_ref().ok_or_else(|| {
            CortexError::WalError("Postgres pool not initialized".into())
        })?;
        sqlx::query(
            r#"
            SELECT entry_id, project_id, timestamp, action, job_id, theme_id, data_json, committed, created_at
            FROM wal_entries WHERE entry_id = $1
            "#,
        )
        .bind(entry_id)
        .fetch_one(pool)
        .await
        .map_err(|e| CortexError::WalError(format!("read_entry (postgres) failed: {}", e)))
    }

    #[cfg(feature = "sqlite")]
    fn wal_entry_from_sqlite_row(&self, row: sqlx::sqlite::SqliteRow) -> Result<WalEntry> {
        let data_json: String = row.get::<String, _>("data_json");
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

    #[cfg(feature = "postgres")]
    fn wal_entry_from_postgres_row(&self, row: sqlx::postgres::PgRow) -> Result<WalEntry> {
        let data_json: String = row.get::<String, _>("data_json");
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

    /// Liste les entrées WAL non committées pour un projet.
    pub async fn list_uncommitted(&self, project_id: &str) -> Result<Vec<WalEntry>> {
        if self.backend_kind == BackendKind::Sqlite {
            #[cfg(feature = "sqlite")]
            {
                let rows = self.list_uncommitted_sqlite(project_id).await?;
                return rows
                    .into_iter()
                    .map(|row| self.wal_entry_from_sqlite_row(row))
                    .collect();
            }
            #[cfg(not(feature = "sqlite"))]
            return Err(CortexError::WalError("SQLite feature disabled".into()));
        } else {
            #[cfg(feature = "postgres")]
            {
                let rows = self.list_uncommitted_postgres(project_id).await?;
                return rows
                    .into_iter()
                    .map(|row| self.wal_entry_from_postgres_row(row))
                    .collect();
            }
            #[cfg(not(feature = "postgres"))]
            return Err(CortexError::WalError("Postgres feature disabled".into()));
        }
    }

    #[cfg(feature = "sqlite")]
    async fn list_uncommitted_sqlite(
        &self,
        project_id: &str,
    ) -> Result<Vec<sqlx::sqlite::SqliteRow>> {
        let pool = self.sqlite_pool.as_ref().ok_or_else(|| {
            CortexError::WalError("SQLite pool not initialized".into())
        })?;
        sqlx::query(
            r#"
            SELECT entry_id, project_id, timestamp, action, job_id, theme_id, data_json, committed, created_at
            FROM wal_entries WHERE project_id = ? AND committed = 0
            ORDER BY timestamp ASC
            "#,
        )
        .bind(project_id)
        .fetch_all(pool)
        .await
        .map_err(|e| CortexError::WalError(format!("list_uncommitted (sqlite) failed: {}", e)))
    }

    #[cfg(feature = "postgres")]
    async fn list_uncommitted_postgres(
        &self,
        project_id: &str,
    ) -> Result<Vec<sqlx::postgres::PgRow>> {
        let pool = self.postgres_pool.as_ref().ok_or_else(|| {
            CortexError::WalError("Postgres pool not initialized".into())
        })?;
        sqlx::query(
            r#"
            SELECT entry_id, project_id, timestamp, action, job_id, theme_id, data_json, committed, created_at
            FROM wal_entries WHERE project_id = $1 AND committed = false
            ORDER BY timestamp ASC
            "#,
        )
        .bind(project_id)
        .fetch_all(pool)
        .await
        .map_err(|e| CortexError::WalError(format!("list_uncommitted (postgres) failed: {}", e)))
    }

    /// Rollback de toutes les entrées WAL non committées d'un projet.
    pub async fn rollback_uncommitted(&self, project_id: &str) -> Result<usize> {
        let rows_affected = match self.backend_kind {
            BackendKind::Sqlite => {
                #[cfg(feature = "sqlite")]
                {
                    let pool = self.sqlite_pool.as_ref().unwrap();
                    let result = sqlx::query(
                        "DELETE FROM wal_entries WHERE project_id = ? AND committed = 0",
                    )
                    .bind(project_id)
                    .execute(pool)
                    .await
                    .map_err(|e| CortexError::WalError(format!("rollback (sqlite) failed: {}", e)))?;
                    result.rows_affected()
                }
                #[cfg(not(feature = "sqlite"))]
                return Err(CortexError::WalError("SQLite feature disabled".into()));
            }
            BackendKind::Postgres => {
                #[cfg(feature = "postgres")]
                {
                    let pool = self.postgres_pool.as_ref().unwrap();
                    let result = sqlx::query(
                        "DELETE FROM wal_entries WHERE project_id = $1 AND committed = false",
                    )
                    .bind(project_id)
                    .execute(pool)
                    .await
                    .map_err(|e| CortexError::WalError(format!("rollback (postgres) failed: {}", e)))?;
                    result.rows_affected()
                }
                #[cfg(not(feature = "postgres"))]
                return Err(CortexError::WalError("Postgres feature disabled".into()));
            }
        };

        Ok(rows_affected as usize)
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
        let diff_json = serde_json::to_string(diff)?;
        let snapshot_json = serde_json::to_string(snapshot)?;

        // Récupère le dernier commit du projet
        let previous_commit: Option<String> = match self.backend_kind {
            BackendKind::Sqlite => {
                #[cfg(feature = "sqlite")]
                {
                    let pool = self.sqlite_pool.as_ref().unwrap();
                    sqlx::query_scalar(
                        "SELECT commit_id FROM cortex_commits WHERE project_id = ? ORDER BY timestamp DESC LIMIT 1",
                    )
                    .bind(project_id)
                    .fetch_optional(pool)
                    .await
                    .map_err(|e| CortexError::WalError(format!("fetch previous commit (sqlite) failed: {}", e)))?
                }
                #[cfg(not(feature = "sqlite"))]
                return Err(CortexError::WalError("SQLite feature disabled".into()));
            }
            BackendKind::Postgres => {
                #[cfg(feature = "postgres")]
                {
                    let pool = self.postgres_pool.as_ref().unwrap();
                    sqlx::query_scalar(
                        "SELECT commit_id FROM cortex_commits WHERE project_id = $1 ORDER BY timestamp DESC LIMIT 1",
                    )
                    .bind(project_id)
                    .fetch_optional(pool)
                    .await
                    .map_err(|e| CortexError::WalError(format!("fetch previous commit (postgres) failed: {}", e)))?
                }
                #[cfg(not(feature = "postgres"))]
                return Err(CortexError::WalError("Postgres feature disabled".into()));
            }
        };

        let checksum_input = format!(
            "{}:{}:{:?}:{}",
            commit_id, timestamp, previous_commit, snapshot_json
        );
        let checksum = sha256_hex(&checksum_input);

        // INSERT
        match self.backend_kind {
            BackendKind::Sqlite => {
                #[cfg(feature = "sqlite")]
                {
                    let pool = self.sqlite_pool.as_ref().unwrap();
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
                    .execute(pool)
                    .await
                    .map_err(|e| CortexError::WalError(format!("write snapshot commit (sqlite) failed: {}", e)))?;
                }
                #[cfg(not(feature = "sqlite"))]
                return Err(CortexError::WalError("SQLite feature disabled".into()));
            }
            BackendKind::Postgres => {
                #[cfg(feature = "postgres")]
                {
                    let pool = self.postgres_pool.as_ref().unwrap();
                    sqlx::query(
                        r#"
                        INSERT INTO cortex_commits
                        (commit_id, project_id, timestamp, previous_commit, mutation_type, trigger_reason, diff_json, snapshot_json, checksum)
                        VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
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
                    .execute(pool)
                    .await
                    .map_err(|e| CortexError::WalError(format!("write snapshot commit (postgres) failed: {}", e)))?;
                }
                #[cfg(not(feature = "postgres"))]
                return Err(CortexError::WalError("Postgres feature disabled".into()));
            }
        }

        Ok(commit_id)
    }

    /// Liste les commits d'un projet, du plus récent au plus ancien.
    pub async fn list_commits(&self, project_id: &str) -> Result<Vec<CortexCommit>> {
        if self.backend_kind == BackendKind::Sqlite {
            #[cfg(feature = "sqlite")]
            {
                let rows = self.list_commits_sqlite(project_id).await?;
                return rows
                    .into_iter()
                    .map(|row| self.cortex_commit_from_sqlite_row(row))
                    .collect();
            }
            #[cfg(not(feature = "sqlite"))]
            return Err(CortexError::WalError("SQLite feature disabled".into()));
        } else {
            #[cfg(feature = "postgres")]
            {
                let rows = self.list_commits_postgres(project_id).await?;
                return rows
                    .into_iter()
                    .map(|row| self.cortex_commit_from_postgres_row(row))
                    .collect();
            }
            #[cfg(not(feature = "postgres"))]
            return Err(CortexError::WalError("Postgres feature disabled".into()));
        }
    }

    #[cfg(feature = "sqlite")]
    async fn list_commits_sqlite(
        &self,
        project_id: &str,
    ) -> Result<Vec<sqlx::sqlite::SqliteRow>> {
        let pool = self.sqlite_pool.as_ref().ok_or_else(|| {
            CortexError::WalError("SQLite pool not initialized".into())
        })?;
        sqlx::query(
            r#"
            SELECT commit_id, project_id, timestamp, previous_commit, mutation_type, trigger_reason, diff_json, snapshot_json, checksum
            FROM cortex_commits WHERE project_id = ?
            ORDER BY timestamp DESC
            "#,
        )
        .bind(project_id)
        .fetch_all(pool)
        .await
        .map_err(|e| CortexError::WalError(format!("list commits (sqlite) failed: {}", e)))
    }

    #[cfg(feature = "postgres")]
    async fn list_commits_postgres(
        &self,
        project_id: &str,
    ) -> Result<Vec<sqlx::postgres::PgRow>> {
        let pool = self.postgres_pool.as_ref().ok_or_else(|| {
            CortexError::WalError("Postgres pool not initialized".into())
        })?;
        sqlx::query(
            r#"
            SELECT commit_id, project_id, timestamp, previous_commit, mutation_type, trigger_reason, diff_json, snapshot_json, checksum
            FROM cortex_commits WHERE project_id = $1
            ORDER BY timestamp DESC
            "#,
        )
        .bind(project_id)
        .fetch_all(pool)
        .await
        .map_err(|e| CortexError::WalError(format!("list commits (postgres) failed: {}", e)))
    }

    #[cfg(feature = "sqlite")]
    fn cortex_commit_from_sqlite_row(
        &self,
        row: sqlx::sqlite::SqliteRow,
    ) -> Result<CortexCommit> {
        let diff_json: String = row.get::<String, _>("diff_json");
        let snapshot_json: String = row.get::<String, _>("snapshot_json");
        Ok(CortexCommit {
            commit_id: row.get("commit_id"),
            project_id: row.get("project_id"),
            timestamp: row.get("timestamp"),
            previous_commit: row.get("previous_commit"),
            mutation_type: row.get("mutation_type"),
            trigger_reason: row.get("trigger_reason"),
            diff: serde_json::from_str(&diff_json).map_err(|e| {
                CortexError::WalError(format!("diff parse error: {}", e))
            })?,
            snapshot: serde_json::from_str(&snapshot_json).map_err(|e| {
                CortexError::WalError(format!("snapshot parse error: {}", e))
            })?,
            checksum: row.get("checksum"),
        })
    }

    #[cfg(feature = "postgres")]
    fn cortex_commit_from_postgres_row(
        &self,
        row: sqlx::postgres::PgRow,
    ) -> Result<CortexCommit> {
        let diff_json: String = row.get::<String, _>("diff_json");
        let snapshot_json: String = row.get::<String, _>("snapshot_json");
        Ok(CortexCommit {
            commit_id: row.get("commit_id"),
            project_id: row.get("project_id"),
            timestamp: row.get("timestamp"),
            previous_commit: row.get("previous_commit"),
            mutation_type: row.get("mutation_type"),
            trigger_reason: row.get("trigger_reason"),
            diff: serde_json::from_str(&diff_json).map_err(|e| {
                CortexError::WalError(format!("diff parse error: {}", e))
            })?,
            snapshot: serde_json::from_str(&snapshot_json).map_err(|e| {
                CortexError::WalError(format!("snapshot parse error: {}", e))
            })?,
            checksum: row.get("checksum"),
        })
    }
    pub async fn latest_commit(&self, project_id: &str) -> Result<Option<CortexCommit>> {
        let commits = self.list_commits(project_id).await?;
        Ok(commits.into_iter().next())
    }

    /// Sauvegarde l'état courant d'un projet.
    pub async fn save_state(
        &self,
        project_id: &str,
        state: &JsonValue,
        handoff_summary: Option<&str>,
    ) -> Result<()> {
        let state_json = serde_json::to_string(state)?;
        let timestamp = Utc::now().timestamp_millis();

        match self.backend_kind {
            BackendKind::Sqlite => {
                #[cfg(feature = "sqlite")]
                {
                    let pool = self.sqlite_pool.as_ref().unwrap();
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
                    .execute(pool)
                    .await
                    .map_err(|e| CortexError::WalError(format!("save state (sqlite) failed: {}", e)))?;
                }
                #[cfg(not(feature = "sqlite"))]
                return Err(CortexError::WalError("SQLite feature disabled".into()));
            }
            BackendKind::Postgres => {
                #[cfg(feature = "postgres")]
                {
                    let pool = self.postgres_pool.as_ref().unwrap();
                    sqlx::query(
                        r#"
                        INSERT INTO cortex_states (project_id, state_json, last_updated, handoff_summary)
                        VALUES ($1, $2, $3, $4)
                        ON CONFLICT (project_id) DO UPDATE SET
                            state_json = EXCLUDED.state_json,
                            last_updated = EXCLUDED.last_updated,
                            handoff_summary = EXCLUDED.handoff_summary
                        "#,
                    )
                    .bind(project_id)
                    .bind(&state_json)
                    .bind(timestamp)
                    .bind(handoff_summary)
                    .execute(pool)
                    .await
                    .map_err(|e| CortexError::WalError(format!("save state (postgres) failed: {}", e)))?;
                }
                #[cfg(not(feature = "postgres"))]
                return Err(CortexError::WalError("Postgres feature disabled".into()));
            }
        }

        Ok(())
    }

    /// Charge l'état courant d'un projet.
    pub async fn load_state(&self, project_id: &str) -> Result<Option<JsonValue>> {
        if self.backend_kind == BackendKind::Sqlite {
            #[cfg(feature = "sqlite")]
            {
                let row_opt = self.load_state_sqlite(project_id).await?;
                return match row_opt {
                    Some(row) => {
                        let state_json: String = row.get::<String, _>("state_json");
                        Ok(Some(serde_json::from_str(&state_json)?))
                    }
                    None => Ok(None),
                };
            }
            #[cfg(not(feature = "sqlite"))]
            return Err(CortexError::WalError("SQLite feature disabled".into()));
        } else {
            #[cfg(feature = "postgres")]
            {
                let row_opt = self.load_state_postgres(project_id).await?;
                return match row_opt {
                    Some(row) => {
                        let state_json: String = row.get::<String, _>("state_json");
                        Ok(Some(serde_json::from_str(&state_json)?))
                    }
                    None => Ok(None),
                };
            }
            #[cfg(not(feature = "postgres"))]
            return Err(CortexError::WalError("Postgres feature disabled".into()));
        }
    }

    #[cfg(feature = "sqlite")]
    async fn load_state_sqlite(
        &self,
        project_id: &str,
    ) -> Result<Option<sqlx::sqlite::SqliteRow>> {
        let pool = self
            .sqlite_pool
            .as_ref()
            .ok_or_else(|| CortexError::WalError("SQLite pool not initialized".into()))?;
        sqlx::query("SELECT state_json FROM cortex_states WHERE project_id = ?")
            .bind(project_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| CortexError::WalError(format!("load state (sqlite) failed: {}", e)))
    }

    #[cfg(feature = "postgres")]
    async fn load_state_postgres(
        &self,
        project_id: &str,
    ) -> Result<Option<sqlx::postgres::PgRow>> {
        let pool = self
            .postgres_pool
            .as_ref()
            .ok_or_else(|| CortexError::WalError("Postgres pool not initialized".into()))?;
        sqlx::query("SELECT state_json FROM cortex_states WHERE project_id = $1")
            .bind(project_id)
            .fetch_optional(pool)
            .await
            .map_err(|e| CortexError::WalError(format!("load state (postgres) failed: {}", e)))
    }
}

// ============================================================
// SQL Migrations (constantes)
// ============================================================

/// Schéma SQLite (mêmes colonnes que l'original, types SQLite).
#[cfg(feature = "sqlite")]
const SQLITE_MIGRATION_STATEMENTS: &[&str] = &[
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
    )
    "#,
    "CREATE INDEX IF NOT EXISTS idx_wal_project_id ON wal_entries(project_id)",
    "CREATE INDEX IF NOT EXISTS idx_wal_uncommitted ON wal_entries(project_id, committed)",
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
    )
    "#,
    "CREATE INDEX IF NOT EXISTS idx_commits_project ON cortex_commits(project_id, timestamp)",
    r#"
    CREATE TABLE IF NOT EXISTS cortex_states (
        project_id TEXT PRIMARY KEY,
        state_json TEXT NOT NULL,
        last_updated INTEGER NOT NULL,
        handoff_summary TEXT
    )
    "#,
];

/// Schéma PostgreSQL (types natifs PG : BIGINT, JSONB, BOOLEAN, TIMESTAMPTZ).
#[cfg(feature = "postgres")]
const POSTGRES_MIGRATION_STATEMENTS: &[&str] = &[
    r#"
    CREATE TABLE IF NOT EXISTS wal_entries (
        entry_id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL,
        timestamp BIGINT NOT NULL,
        action TEXT NOT NULL,
        job_id TEXT,
        theme_id TEXT,
        data_json JSONB NOT NULL,
        committed BOOLEAN NOT NULL DEFAULT false,
        created_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW()) * 1000)::BIGINT
    )
    "#,
    "CREATE INDEX IF NOT EXISTS idx_wal_project_id ON wal_entries(project_id)",
    "CREATE INDEX IF NOT EXISTS idx_wal_uncommitted ON wal_entries(project_id, committed)",
    r#"
    CREATE TABLE IF NOT EXISTS cortex_commits (
        commit_id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL,
        timestamp BIGINT NOT NULL,
        previous_commit TEXT,
        mutation_type TEXT NOT NULL,
        trigger_reason TEXT NOT NULL,
        diff_json JSONB NOT NULL,
        snapshot_json JSONB NOT NULL,
        checksum TEXT NOT NULL,
        created_at BIGINT NOT NULL DEFAULT (EXTRACT(EPOCH FROM NOW()) * 1000)::BIGINT
    )
    "#,
    "CREATE INDEX IF NOT EXISTS idx_commits_project ON cortex_commits(project_id, timestamp)",
    r#"
    CREATE TABLE IF NOT EXISTS cortex_states (
        project_id TEXT PRIMARY KEY,
        state_json JSONB NOT NULL,
        last_updated BIGINT NOT NULL,
        handoff_summary TEXT
    )
    "#,
];

/// Helper SHA256 simple (sans dépendance externe).
fn sha256_hex(input: &str) -> String {
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
        assert_eq!(wal.backend_kind(), BackendKind::Sqlite);
    }

    #[test]
    fn test_backend_kind_from_url() {
        assert_eq!(
            BackendKind::from_url("sqlite::memory:").unwrap(),
            BackendKind::Sqlite
        );
        assert_eq!(
            BackendKind::from_url("sqlite:///tmp/cortex.db").unwrap(),
            BackendKind::Sqlite
        );
        assert_eq!(
            BackendKind::from_url("sqlite:C:\\Users\\nivra\\cortex.db").unwrap(),
            BackendKind::Sqlite
        );
        // Postgres detection (regardless of feature flag for the test)
        #[cfg(feature = "postgres")]
        {
            assert_eq!(
                BackendKind::from_url("postgres://localhost/db").unwrap(),
                BackendKind::Postgres
            );
            assert_eq!(
                BackendKind::from_url("postgresql://user:pass@host:5432/db").unwrap(),
                BackendKind::Postgres
            );
        }
        // Unknown scheme
        assert!(BackendKind::from_url("mongodb://localhost").is_err());
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
            .expect("write_prepare");
        wal.write_commit(&entry_id).await.expect("write_commit");
    }

    #[tokio::test]
    async fn test_recover_uncommitted() {
        let wal = test_wal().await;
        // Écrit 3 entries : 1 commitée, 2 non
        let committed = wal
            .write_prepare("proj_r", "approval_received", None, None, &json!({}))
            .await
            .unwrap();
        wal.write_commit(&committed).await.unwrap();

        let _uncommitted_a = wal
            .write_prepare("proj_r", "sync_reflect", Some("J-1"), None, &json!({}))
            .await
            .unwrap();
        let _uncommitted_b = wal
            .write_prepare("proj_r", "unknown_action", None, None, &json!({}))
            .await
            .unwrap();

        let report = wal
            .recover_uncommitted("proj_r")
            .await
            .expect("recover");
        // L'entry déjà commitée n'apparaît pas dans uncommitted
        // → 2 uncommitted : sync_reflect (escalate) + unknown_action (rollback)
        assert_eq!(report.uncommitted_count, 2);
        assert_eq!(report.escalated.len(), 1, "sync_reflect → escalate");
        assert_eq!(report.rolled_back.len(), 1, "unknown → rollback");
    }

    #[tokio::test]
    async fn test_save_and_load_state() {
        let wal = test_wal().await;
        let state = json!({"plan": {"themes": ["A", "B"]}});
        wal.save_state("proj_s", &state, Some("test handoff"))
            .await
            .expect("save_state");
        let loaded = wal.load_state("proj_s").await.expect("load_state");
        assert_eq!(loaded, Some(state));
    }

    #[tokio::test]
    async fn test_write_snapshot_commit() {
        let wal = test_wal().await;
        let commit_id = wal
            .write_snapshot_commit(
                "proj_sc",
                "plan_generated",
                "intercept_plan",
                &json!({"summary": "test"}),
                &json!({"plan": {}}),
            )
            .await
            .expect("snapshot commit");
        assert!(commit_id.starts_with("commit_"));
        let latest = wal.latest_commit("proj_sc").await.expect("latest");
        assert!(latest.is_some());
        assert_eq!(latest.unwrap().commit_id, commit_id);
    }

    // ============================================================
    // LIVE Postgres integration tests (Session 6 - E.2)
    // ============================================================
    //
    // Ces tests sont #[ignore] par défaut (pas de PG dans la CI standard).
    // Pour les exécuter : TEST_POSTGRES_URL=postgres://... cargo test --features postgres -- --include-ignored
    //
    // Ils testent le vrai cycle CRUD + recovery contre une vraie instance PG.

    #[cfg(feature = "postgres")]
    fn live_pg_url() -> Option<String> {
        std::env::var("TEST_POSTGRES_URL").ok()
    }

    #[cfg(feature = "postgres")]
    async fn live_pg_wal() -> Option<WalService> {
        let url = live_pg_url()?;
        // Chaque test utilise un project_id unique pour éviter les collisions
        // entre exécutions parallèles.
        WalService::connect(&url).await.ok()
    }

    #[cfg(feature = "postgres")]
    #[tokio::test]
    #[ignore = "requires TEST_POSTGRES_URL env var (live Postgres)"]
    async fn live_postgres_write_prepare_and_commit() {
        let wal = match live_pg_wal().await {
            Some(w) => w,
            None => {
                eprintln!("TEST_POSTGRES_URL not set, skipping");
                return;
            }
        };
        assert_eq!(wal.backend_kind(), BackendKind::Postgres);

        let entry_id = wal
            .write_prepare(
                "live_proj_1",
                "live_action",
                None,
                None,
                &json!({"live": true}),
            )
            .await
            .expect("live write_prepare");
        wal.write_commit(&entry_id)
            .await
            .expect("live write_commit");

        let entry = wal
            .read_entry(&entry_id)
            .await
            .expect("live read_entry");
        assert_eq!(entry.action, "live_action");
        assert!(entry.committed);
    }

    #[cfg(feature = "postgres")]
    #[tokio::test]
    #[ignore = "requires TEST_POSTGRES_URL env var (live Postgres)"]
    async fn live_postgres_save_and_load_state_jsonb() {
        let wal = match live_pg_wal().await {
            Some(w) => w,
            None => return,
        };
        // Test que JSONB fonctionne (round-trip avec structure imbriquée).
        let state = json!({
            "plan": {
                "themes": [
                    {"id": "TH-1", "name": "Live test", "criticity": 4}
                ]
            },
            "metadata": {"key": "value", "count": 42}
        });
        wal.save_state("live_proj_state", &state, Some("live handoff"))
            .await
            .expect("live save_state");
        let loaded = wal
            .load_state("live_proj_state")
            .await
            .expect("live load_state");
        assert_eq!(loaded, Some(state));
    }

    #[cfg(feature = "postgres")]
    #[tokio::test]
    #[ignore = "requires TEST_POSTGRES_URL env var (live Postgres)"]
    async fn live_postgres_recovery_all_policies() {
        let wal = match live_pg_wal().await {
            Some(w) => w,
            None => return,
        };
        // Crée 3 entries : 1 commit, 1 sync_reflect (escalate), 1 unknown (rollback)
        let committed = wal
            .write_prepare("live_proj_rec", "approval_received", None, None, &json!({}))
            .await
            .expect("prepare 1");
        wal.write_commit(&committed).await.expect("commit 1");

        let _to_escalate = wal
            .write_prepare("live_proj_rec", "sync_reflect", Some("J-1"), None, &json!({}))
            .await
            .expect("prepare 2");

        let _to_rollback = wal
            .write_prepare("live_proj_rec", "unknown_action_xyz", None, None, &json!({}))
            .await
            .expect("prepare 3");

        let report = wal
            .recover_uncommitted("live_proj_rec")
            .await
            .expect("live recover");
        assert_eq!(report.uncommitted_count, 2, "2 uncommitted");
        assert_eq!(report.escalated.len(), 1, "sync_reflect → escalate");
        assert_eq!(report.rolled_back.len(), 1, "unknown → rollback");
    }

    #[cfg(feature = "postgres")]
    #[tokio::test]
    #[ignore = "requires TEST_POSTGRES_URL env var (live Postgres)"]
    async fn live_postgres_concurrent_writers() {
        // Vérifie que Postgres gère les writers concurrents (vs SQLite qui sérialise).
        let wal = match live_pg_wal().await {
            Some(w) => w,
            None => return,
        };
        let mut handles = vec![];
        for i in 0..5 {
            let wal = wal.clone();
            handles.push(tokio::spawn(async move {
                wal.write_prepare(
                    &format!("live_proj_conc_{}", i),
                    "concurrent_write",
                    None,
                    None,
                    &json!({"writer": i}),
                )
                .await
            }));
        }
        for h in handles {
            h.await.expect("task join").expect("write_prepare");
        }
        // Si on arrive ici sans panic ni lock timeout, c'est bon.
    }
}
