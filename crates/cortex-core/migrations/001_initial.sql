-- Migration initiale Cortex MCP
-- Crée les tables nécessaires pour le Write-Ahead Log et snapshots

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
CREATE INDEX IF NOT EXISTS idx_wal_timestamp ON wal_entries(project_id, timestamp);

-- Table pour les commits (snapshots d'état)
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
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    FOREIGN KEY (previous_commit) REFERENCES cortex_commits(commit_id)
);

CREATE INDEX IF NOT EXISTS idx_commits_project ON cortex_commits(project_id, timestamp);

-- Table pour l'état courant des projets (snapshots rapides)
CREATE TABLE IF NOT EXISTS cortex_states (
    project_id TEXT PRIMARY KEY,
    state_json TEXT NOT NULL,
    last_updated INTEGER NOT NULL,
    handoff_summary TEXT
);
