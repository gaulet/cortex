# 🚀 DEPLOY.md — Guide de déploiement Cortex MCP en production

> Comment mettre Cortex MCP en service en mode PostgreSQL.
> Pour le dev local avec SQLite, voir plutôt [CORTEX-USER-GUIDE.md](./CORTEX-USER-GUIDE.md).

---

## 1. Prérequis

| Composant | Version minimum | Pourquoi |
|-----------|----------------|----------|
| PostgreSQL | **14+** (16 recommandé) | `JSONB` + `EXTRACT(EPOCH FROM NOW())` utilisé dans le schéma |
| Rust | 1.75+ stable | Pour compiler le binaire |
| OS | Linux / macOS / Windows | Cross-platform |
| RAM | 256 MB par instance cortex-mcp | Footprint minimal |

PostgreSQL 14 minimum car on utilise des patterns SQL modernes :
- `JSONB` (natif depuis PG 9.4, on en tire parti pour le state)
- `BIGINT GENERATED ALWAYS AS IDENTITY` (PG 10+)
- Index partiels (PG 11+)

---

## 2. Setup PostgreSQL

### 2.1. Avec Docker (dev/test/staging)

```bash
# Depuis la racine du repo cortex-mcp
cd ~/Documents/Cortex/cortex-mcp

# Démarrer Postgres 16
docker compose up -d

# Vérifier qu'il est prêt
docker compose ps
# STATUS: Up (healthy)

# (Optionnel) Démarrer pgAdmin pour explorer la DB
docker compose --profile ui up -d
# UI accessible sur http://localhost:8080 (admin@cortex.local / admin)
```

Connection string :
```
postgres://cortex:cortex_dev_pw@localhost:5432/cortex_db
```

### 2.2. Avec un PostgreSQL managé (prod recommandée)

Pour la prod, utiliser un service managé :
- **AWS RDS** : `postgres://cortex:<password>@<endpoint>.rds.amazonaws.com:5432/cortex_db`
- **Google Cloud SQL** : idem
- **Supabase** : `postgres://postgres.<project>:<password>@aws-0-<region>.pooler.supabase.com:6543/postgres`
- **Self-hosted** : `postgres://cortex:<password>@pg.internal:5432/cortex_db`

**Recommandations prod** :
- Backup automatique PITR activé (RDS le fait par défaut)
- Connexion via TLS (`?sslmode=require` dans l'URL)
- Au moins 2 vCPU, 4 GB RAM pour < 100 projets actifs
- Connection pooler (PgBouncer) si > 50 connexions simultanées

### 2.3. Création manuelle de la DB et du user

Si tu installes PostgreSQL toi-même :

```sql
-- En tant que superuser (postgres)
CREATE USER cortex WITH PASSWORD 'CHANGEME_STRONG_PASSWORD';
CREATE DATABASE cortex_db OWNER cortex;
GRANT ALL PRIVILEGES ON DATABASE cortex_db TO cortex;

-- Connexion à cortex_db puis :
GRANT ALL ON SCHEMA public TO cortex;
```

⚠️ **Change le password** avant de mettre en prod. Idéalement, utilise un secret manager (Vault, AWS Secrets Manager) plutôt qu'un literal.

---

## 3. Compiler cortex-mcp avec le support PostgreSQL

```bash
cd ~/Documents/Cortex/cortex-mcp

# Build release avec feature postgres (compilation plus longue car ajoute sqlx/postgres)
cargo build --release \
  -p cortex-mcp-server \
  --no-default-features \
  --features cortex-core/postgres
```

Le binaire se trouve dans `target/release/cortex-mcp.exe` (Windows) ou `target/release/cortex-mcp` (Unix). Taille ≈ 10.8 MB.

**Note** : le binaire par défaut (sans `--features`) est compilé en mode SQLite. Pour passer en mode Postgres, il **faut** recompiler avec la feature.

---

## 4. Configuration runtime

### 4.1. Variables d'environnement minimales

```bash
# Requis : URL de la DB Postgres
export CORTEX_WAL_URL="postgres://cortex:YOUR_PASSWORD@db.internal:5432/cortex_db"

# Requis en prod : clé API LLM (provider openai-compatible)
export CORTEX_LLM_PROVIDER=openai
export CORTEX_LLM_API_KEY=sk-or-v1-...  # OpenRouter ou autre
export CORTEX_LLM_BASE_URL=https://openrouter.ai/api/v1
export CORTEX_LLM_MODEL=minimax/minimax-m3
export CORTEX_LLM_REASONING=max

# Recommandé en prod : HMAC signing des guardrails
export CORTEX_HMAC_SECRET=$(openssl rand -hex 32)

# Optionnel : timeout LLM (default 60s)
export CORTEX_LLM_TIMEOUT=120
```

### 4.2. Lancement

```bash
# Foreground (debug)
./target/release/cortex-mcp

# Background avec logs
nohup ./target/release/cortex-mcp > /var/log/cortex.log 2>&1 &
```

Le serveur parle MCP en `stdio` (JSON-RPC 2.0 sur stdin/stdout). Pour l'utiliser avec Hermes, configurer dans `~/.hermes/config.yaml` :

```yaml
mcp:
  servers:
    cortex:
      command: "/opt/cortex/cortex-mcp"
      env:
        CORTEX_WAL_URL: "postgres://cortex:***@db.internal:5432/cortex_db"
        CORTEX_LLM_PROVIDER: "openai"
        CORTEX_LLM_API_KEY: "sk-or-v1-..."
        CORTEX_LLM_BASE_URL: "https://openrouter.ai/api/v1"
        CORTEX_LLM_MODEL: "minimax/minimax-m3"
        CORTEX_LLM_REASONING: "max"
        CORTEX_HMAC_SECRET: "votre-secret-hmac"
```

---

## 5. Tests d'intégration live (recommandé en CI)

### 5.1. Setup dans GitHub Actions

```yaml
# .github/workflows/cortex-postgres-integration.yml
name: Cortex + Postgres Integration

on: [push, pull_request]

jobs:
  integration:
    runs-on: ubuntu-latest
    services:
      postgres:
        image: postgres:16
        env:
          POSTGRES_USER: cortex_test
          POSTGRES_PASSWORD: cortex_test_pw
          POSTGRES_DB: cortex_test_db
        ports:
          - 5432:5432
        options: >-
          --health-cmd "pg_isready -U cortex_test"
          --health-interval 10s
          --health-timeout 5s
          --health-retries 5
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - name: Build with postgres
        run: |
          cargo build --release -p cortex-core --no-default-features --features postgres
      - name: Run integration tests
        env:
          TEST_POSTGRES_URL: postgres://cortex_test:cortex_test_pw@localhost:5432/cortex_test_db
        run: |
          cargo test -p cortex-core --features postgres -- --include-ignored
```

### 5.2. Lancer les tests en local

```bash
# 1. Démarrer Postgres
docker compose up -d

# 2. Définir l'URL de test
export TEST_POSTGRES_URL="postgres://cortex:cortex_dev_pw@localhost:5432/cortex_db"

# 3. Build avec feature postgres
cargo build -p cortex-core --features postgres

# 4. Lancer les tests (les tests live sont #[ignore] par défaut)
cargo test -p cortex-core --features postgres -- --include-ignored
```

Les tests live sont marqués `#[ignore]` pour ne pas s'exécuter en CI sans Postgres. Avec `--include-ignored`, ils s'exécutent et parlent à la vraie DB.

---

## 6. Monitoring PostgreSQL

### 6.1. Requêtes utiles

#### Connexions actives par cortex-mcp
```sql
SELECT
    pid,
    usename,
    application_name,
    client_addr,
    state,
    query_start,
    LEFT(query, 100) AS query_preview
FROM pg_stat_activity
WHERE datname = 'cortex_db'
  AND application_name LIKE '%cortex%'
ORDER BY query_start DESC;
```

#### Locks en attente (deadlock detection)
```sql
SELECT
    blocked_locks.pid AS blocked_pid,
    blocked_activity.usename AS blocked_user,
    blocking_locks.pid AS blocking_pid,
    blocking_activity.usename AS blocking_user,
    blocked_activity.query AS blocked_statement,
    blocking_activity.query AS blocking_statement
FROM pg_catalog.pg_locks blocked_locks
JOIN pg_catalog.pg_stat_activity blocked_activity
    ON blocked_activity.pid = blocked_locks.pid
JOIN pg_catalog.pg_locks blocking_locks
    ON blocking_locks.locktype = blocked_locks.locktype
    AND blocking_locks.pid != blocked_locks.pid
JOIN pg_catalog.pg_stat_activity blocking_activity
    ON blocking_activity.pid = blocking_locks.pid
WHERE NOT blocked_locks.granted;
```

#### Taille des tables cortex
```sql
SELECT
    schemaname,
    relname AS table_name,
    pg_size_pretty(pg_total_relation_size(relid)) AS total_size,
    pg_size_pretty(pg_relation_size(relid)) AS table_size,
    pg_size_pretty(pg_indexes_size(relid)) AS indexes_size,
    n_live_tup AS row_count
FROM pg_stat_user_tables
WHERE schemaname = 'public'
ORDER BY pg_total_relation_size(relid) DESC;
```

#### Queries lentes (top 10 sur 1h)
```sql
SELECT
    query,
    calls,
    mean_exec_time AS avg_ms,
    max_exec_time AS max_ms
FROM pg_stat_statements
WHERE query LIKE '%cortex%'
ORDER BY mean_exec_time DESC
LIMIT 10;
```
*Note : nécessite `pg_stat_statements` activé dans `postgresql.conf`.*

#### Taux de commit WAL (sanity check)
```sql
SELECT
    pg_size_pretty(pg_wal_lsn_diff(pg_current_wal_lsn(), '0/0')) AS total_wal_generated;
```

### 6.2. Métriques Cortex elles-mêmes

Cortex expose ses propres métriques via l'outil MCP `get_metrics` (12 compteurs + 8 histogrammes Prometheus). Configurer Prometheus pour scraper :

```yaml
# prometheus.yml
scrape_configs:
  - job_name: 'cortex-mcp'
    static_configs:
      - targets: ['cortex.internal:9100']  # exposer /metrics via HTTP (TODO)
```

**Note** : pour l'instant, `get_metrics` est exposé en JSON-RPC (pas HTTP). Pour Prometheus scraping direct, il faut ajouter un endpoint HTTP `/metrics` dans `cortex-mcp-server`. **À faire dans une session dédiée** (option B - webhooks pourrait inclure ça).

### 6.3. Alertes recommandées (Grafana / Alertmanager)

```yaml
# alerts.yml
groups:
  - name: cortex
    rules:
      - alert: CortexHighLatency
        expr: histogram_quantile(0.95, rate(cortex_intercept_plan_duration_seconds_bucket[5m])) > 10
        for: 5m
        annotations:
          summary: "Cortex intercept_plan p95 latency > 10s"

      - alert: CortexRecoveryFrequent
        expr: rate(cortex_recovery_rolled_back_total[5m]) > 0.1
        for: 10m
        annotations:
          summary: "Cortex rollback rate > 0.1/s (investigate crashes)"

      - alert: PostgresConnectionsHigh
        expr: count(pg_stat_activity{datname="cortex_db"}) > 80
        for: 2m
        annotations:
          summary: "Postgres connections > 80 (close to max_connections)"
```

---

## 7. Migrations de schéma

Cortex crée automatiquement les 3 tables au démarrage via `run_migrations()` :
- `wal_entries` (préparation/commit du WAL)
- `cortex_commits` (snapshots d'état)
- `cortex_states` (cache état courant)

Les migrations sont **idempotentes** (`CREATE TABLE IF NOT EXISTS`). Pas de système de version de schéma pour l'instant — si tu changes le schéma, ajoute un nouveau `CREATE INDEX IF NOT EXISTS` à la fin de `POSTGRES_MIGRATION_STATEMENTS` dans `wal.rs`.

**Pour l'avenir** (TODO) : passer à `sqlx::migrate!()` avec un dossier `migrations/` versionné, comme dans la plupart des projets Rust sérieux.

---

## 8. Backup et disaster recovery

### 8.1. Backup quotidien (pg_dump)

```bash
# Backup complet
pg_dump -h db.internal -U cortex -d cortex_db -F c -f /backup/cortex-$(date +%Y%m%d).dump

# Restore
pg_restore -h db.internal -U cortex -d cortex_db -c /backup/cortex-20260604.dump
```

### 8.2. PITR (Point In Time Recovery)

RDS et la plupart des services managés le font nativement. Si self-hosted :

```bash
# postgresql.conf
wal_level = replica
archive_mode = on
archive_command = 'cp %p /archive/%f'

# Restore à un timestamp précis
recovery.conf (PG ≤ 11) ou postgresql.auto.conf (PG 12+) :
recovery_target_time = '2026-06-04 14:30:00'
```

### 8.3. Test de restore (à faire régulièrement)

```bash
# Tous les dimanches
pg_restore -h test-restore.internal -U cortex -d cortex_test /backup/cortex-latest.dump
# Vérifier que les données sont cohérentes
psql -h test-restore.internal -U cortex -d cortex_test \
  -c "SELECT COUNT(*) FROM wal_entries; SELECT COUNT(*) FROM cortex_commits;"
```

---

## 9. Checklist de mise en prod

- [ ] PostgreSQL ≥ 14 provisionné (managé ou self-hosted)
- [ ] User `cortex` créé avec password fort
- [ ] Database `cortex_db` créée
- [ ] TLS activé sur la connexion PG (`sslmode=require`)
- [ ] Backups automatiques configurés (quotidien minimum)
- [ ] PITR activé (recommandé)
- [ ] cortex-mcp compilé en release avec `--features cortex-core/postgres`
- [ ] Variables d'environnement définies (CORTEX_WAL_URL, CORTEX_LLM_*, CORTEX_HMAC_SECRET)
- [ ] HMAC_SECRET stocké dans un secret manager (pas en clair)
- [ ] Tests d'intégration live passent en CI
- [ ] Prometheus scrape configuré (si métriques externes)
- [ ] Alertes Grafana configurées
- [ ] Runbook d'incident documenté (qui appeler si PG down, etc.)

---

## 10. Troubleshooting

| Symptôme | Cause probable | Solution |
|----------|---------------|----------|
| `Failed to connect WAL` | PG pas démarré / firewall | `pg_isready -h host -p 5432` |
| `password authentication failed` | Mauvais user/password | Vérifier `CORTEX_WAL_URL` |
| `database "cortex_db" does not exist` | DB pas créée | `CREATE DATABASE cortex_db` |
| `permission denied for table wal_entries` | User sans droits | `GRANT ALL ON ALL TABLES IN SCHEMA public TO cortex` |
| Latence > 30s sur intercept_plan | PG surchargé ou LLM lent | Vérifier `pg_stat_activity`, `pg_stat_statements` |
| Binaire crash au démarrage | Pas compilé avec `--features postgres` | Recompiler avec la feature |
| Binaire compile mais utilise SQLite | URL commence par `sqlite:` au lieu de `postgres:` | Corriger `CORTEX_WAL_URL` |

---

## Liens utiles

- [PostgreSQL documentation](https://www.postgresql.org/docs/16/)
- [sqlx (crate utilisé)](https://docs.rs/sqlx/latest/sqlx/)
- [CORTEX-USER-GUIDE.md](./CORTEX-USER-GUIDE.md) (guide dev avec SQLite)
- [specification-cortex-v2.md](../planning/specification-cortex-v2.md) (spec complète du projet)
