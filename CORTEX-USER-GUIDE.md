# Cortex MCP Server — User Guide

> Brain-as-MCP pour Hermes Agent : planification fractale, Pre-Mortem, Red-Team, recovery crash, métriques Prometheus.

---

## 1. Vue d'ensemble

**Cortex** est un serveur MCP (Model Context Protocol) écrit en Rust qui agit comme **cerveau stratégique** pour Hermes Agent. Il complète le cycle d'exécution de Hermes en fournissant :

- **Planification fractale** : décomposition d'une intention utilisateur en thèmes parallélisables avec dependencies
- **Pre-Mortem** : génération de guardrails anti-échec (catalogue F1-F19) pour jobs à criticité ≥ 4
- **Red-Team audit** : validation adversariale 5 couches (HMAC integrity, DoD, guardrails, convergence, edge cases)
- **Worker lifecycle** : dispatch, validation, sync_reflect, rollback, abort
- **Crash recovery** : `WAL` SQLite + recovery automatique des entries non-committées
- **HMAC anti-tampering** : signature des guardrails pour empêcher les workers de les modifier
- **Observabilité Prometheus** : 13 compteurs exposés via l'outil `get_metrics`

Cortex communique avec Hermes **uniquement** via le protocole MCP (stdio JSON-RPC 2.0). Pas de filesystem partagé, pas d'état partagé : tout passe par les outils MCP.

---

## 2. Installation

### Pré-requis
- **Rust 1.70+** (toolchain stable)
- **SQLite 3** (inclus via `sqlx`)
- **OpenSSL** (pour reqwest, dependency de `HttpLlmClient`)

### Build
```bash
cd ~/Documents/Cortex/cortex-mcp
cargo build --release
```

Le binaire est compilé dans `target/release/cortex-mcp` (Linux/Mac) ou `target/release/cortex-mcp.exe` (Windows).

### Vérification
```bash
$ ./target/release/cortex-mcp --version
cortex-mcp 0.1.0
```

---

## 3. Configuration via variables d'environnement

Toutes les variables sont préfixées par `CORTEX_`. Aucune n'est obligatoire : le serveur démarre en mode `mock LLM` + `SQLite in-memory` si rien n'est défini.

| Variable | Default | Description |
|---|---|---|
| `CORTEX_WAL_URL` | `sqlite::memory:` | URL de la base WAL. **SQLite** (défaut) : `sqlite://chemin/vers/db?mode=rwc` ou `sqlite::memory:`. **PostgreSQL** (feature requise) : `postgres://user:pass@host:5432/dbname` (ex : `postgres://cortex:***@localhost:5432/cortex`). Le backend est détecté automatiquement depuis le préfixe. |
| `CORTEX_LLM_PROVIDER` | `mock` | `mock` (tests) ou `openai` (compatible OpenAI) |
| `CORTEX_LLM_API_KEY` | — | Clé API LLM. **Requis si provider=openai** |
| `CORTEX_LLM_BASE_URL` | `https://api.openai.com/v1` | URL de base. Pour OpenRouter : `https://openrouter.ai/api/v1`. Pour ollama local : `http://localhost:11434/v1` |
| `CORTEX_LLM_MODEL` | `gpt-4o-mini` | Modèle. Exemples : `anthropic/claude-sonnet-4`, `minimax/minimax-m3`, `llama3.1-70b` |
| `CORTEX_LLM_REASONING` | (vide) | Active le reasoning étendu. Valeurs : `low` / `medium` / `high` / `max`. Injecté comme `reasoning: { effort: "max" }` dans la requête |
| `CORTEX_LLM_TIMEOUT` | `60` | Timeout en secondes pour les appels LLM |
| `CORTEX_HMAC_SECRET` | (vide) | Secret HMAC-SHA256 (32+ bytes) pour signer les guardrails Pre-Mortem. Vide = pas de signature (mode dégradé) |

### Exemple : configuration MiniMax M3 max reasoning
```bash
export CORTEX_LLM_PROVIDER=openai
export CORTEX_LLM_API_KEY=sk-or-v1-...
export CORTEX_LLM_BASE_URL=https://openrouter.ai/api/v1
export CORTEX_LLM_MODEL=minimax/minimax-m3
export CORTEX_LLM_REASONING=max
export CORTEX_HMAC_SECRET=$(openssl rand -hex 32)
export CORTEX_WAL_URL="sqlite:///var/lib/cortex/cortex.db?mode=rwc"
```

### Exemple : configuration Windows (PowerShell)
```powershell
$env:CORTEX_LLM_PROVIDER = "openai"
$env:CORTEX_LLM_API_KEY = "sk-or-v1-..."
$env:CORTEX_LLM_BASE_URL = "https://openrouter.ai/api/v1"
$env:CORTEX_LLM_MODEL = "minimax/minimax-m3"
$env:CORTEX_LLM_REASONING = "max"
$env:CORTEX_HMAC_SECRET = -join ((1..32) | ForEach-Object { '{0:x2}' -f (Get-Random -Maximum 256) })
$env:CORTEX_WAL_URL = "sqlite://C:\Users\nivra\AppData\Local\cortex\cortex.db?mode=rwc"
```

---

## 4. Branchement dans Hermes

Cortex expose un serveur MCP via `stdio` (compatible Windows, Mac, Linux, WSL2, Docker). Il est démarré par Hermes comme un sous-processus.

### Configuration `~/.hermes/config.yaml`
```yaml
mcp:
  servers:
    cortex:
      command: "C:\\Users\\nivra\\Documents\\Cortex\\cortex-mcp\\target\\release\\cortex-mcp.exe"
      args: []
      env:
        CORTEX_LLM_PROVIDER: "openai"
        CORTEX_LLM_API_KEY: "sk-or-v1-..."
        CORTEX_LLM_BASE_URL: "https://openrouter.ai/api/v1"
        CORTEX_LLM_MODEL: "minimax/minimax-m3"
        CORTEX_LLM_REASONING: "max"
        CORTEX_HMAC_SECRET: "your-32-byte-hex-secret"
        CORTEX_WAL_URL: "sqlite://C:\\Users\\nivra\\AppData\\Local\\cortex\\cortex.db?mode=rwc"
      transport: "stdio"
```

**Note** : le fichier `hermes-config-cortex-snippet.yaml` est fourni comme template dans `~/Documents/Cortex/planning/`.

### Vérification du branchement
Démarrez Hermes en mode interactif, puis demandez :
```
> Liste les outils MCP disponibles
```
Vous devez voir les 12 outils `cortex_*` listés (voir section 5).

---

## 5. Outils MCP exposés (12/12)

### 5.1 Planification

#### `get_routing_rules`
Retourne les règles de routage que Cortex applique pour décider si une intention doit être planifiée ou routée directement vers Hermes.

```json
// Appel
{"method": "tools/call", "params": {"name": "get_routing_rules", "arguments": {}}}

// Réponse (extrait)
{
  "intent_classification_rules": {
    "code_refactor": {"min_complexity": 3, "auto_route_to_cortex": true},
    "simple_question": {"min_complexity": 0, "auto_route_to_cortex": false}
  }
}
```

#### `intercept_plan`
**Outil principal de planification**. Prend une intention utilisateur, retourne un `FractalPlan` (thèmes parallélisables + tasks par thème + criticité).

```json
// Appel
{
  "method": "tools/call",
  "params": {
    "name": "intercept_plan",
    "arguments": {
      "intent": "Refactorise le module auth en microservices",
      "context": "FastAPI, PostgreSQL 15, 50k users",
      "project_id": null
    }
  }
}

// Réponse (extrait)
{
  "project_id": "project-0193f4a2-...",
  "plan": {
    "themes": [
      {
        "id": "theme-1",
        "name": "Extracter auth-service",
        "criticity_score": 4,
        "guardrails_request": true,
        "red_team_audit_request": true,
        "tasks": [
          {"id": "T-1.1", "description": "Créer auth-service scaffold", "estimated_tokens": 1500}
        ],
        "depends_on": []
      }
    ]
  },
  "requires_user_approval": true,
  "summary_for_user": "3 thèmes, 7 tasks, criticité max 4 (Pre-Mortem activé)"
}
```

#### `approve_and_execute`
Une fois le plan approuvé par l'utilisateur, génère l'ordre de dispatch ordonné par **phases parallélisables** (topological sort via algorithme de Kahn).

```json
// Appel
{
  "method": "tools/call",
  "params": {
    "name": "approve_and_execute",
    "arguments": {
      "project_id": "project-0193f4a2-...",
      "plan": {...},  // optionnel : si null, lit depuis WAL state cache
      "approved_by": "user"
    }
  }
}

// Réponse
{
  "dispatch_order": {
    "phases": [
      ["theme-1", "theme-2"],  // Phase 1 : 2 thèmes en parallèle
      ["theme-3"]              // Phase 2 : dépend du thème-1
    ],
    "total_themes": 3,
    "total_estimated_tokens": 7500,
    "cost_gating_summary": "..."
  },
  "instructions": "Phase 1 (2 thèmes en parallèle)..."
}
```

### 5.2 Audit et validation

#### `pre_mortem`
Génère des **guardrails** pour un job à criticité ≥ 4. Si `CORTEX_HMAC_SECRET` est configuré, les guardrails sont **signés HMAC** pour empêcher les workers de les modifier.

```json
// Appel
{
  "method": "tools/call",
  "params": {
    "name": "pre_mortem",
    "arguments": {
      "job_id": "J-1.1",
      "job_description": "Migrate users table to new schema",
      "definition_of_done": "Schema migrated + tests pass + zero data loss",
      "context": "PostgreSQL 15, 50M rows, no downtime"
    }
  }
}

// Réponse
{
  "job_id": "J-1.1",
  "guardrails": [
    {
      "id": "G-1.1",
      "description": "Backup table before migration",
      "failure_mode": "F7",  // Référence au catalogue F1-F19
      "check_command": "test -f /backup/users-$(date +%Y%m%d).sql"
    },
    ...
  ],
  "risk_assessment": "high",
  "estimated_risk_score": 4
}
```

#### `red_team_audit`
Audit adversarial **5 couches** d'un artéfact produit par un worker.

```json
// Appel
{
  "method": "tools/call",
  "params": {
    "name": "red_team_audit",
    "arguments": {
      "job_id": "J-1.1",
      "definition_of_done": "Tests pass + coverage ≥ 80%",
      "guardrails": [...],  // reçus de pre_mortem
      "guardrails_signature": "a3f2e8...",  // signature HMAC reçue
      "artifact": "diff --git a/auth.py b/auth.py ..."
    }
  }
}

// Réponse
{
  "passed": false,
  "approved_layers": ["hmac_integrity", "guardrails"],
  "summary": "Coverage 65% < 80% requis",
  "issues": [
    {
      "layer": "definition_of_done",
      "severity": "critical",
      "description": "Coverage 65% < 80% requis",
      "suggestion": "Ajouter tests pour extract_token() et validate_jwt()"
    }
  ]
}
```

Les 5 couches auditées : (1) HMAC integrity, (2) Definition of Done, (3) Executable guardrails, (4) Workflow coherence, (5) Edge cases attack.

#### `sync_reflect`
**Wrapper de Hermes** : à appeler après chaque job worker terminé. Orchestre : red_team_audit → décision (commit/retry/escalate) → log WAL.

```json
// Appel
{
  "method": "tools/call",
  "params": {
    "name": "sync_reflect",
    "arguments": {
      "project_id": "project-0193f4a2-...",
      "job_id": "J-1.1",
      "artifact": "...",
      "definition_of_done": "...",
      "guardrails_signature": "..."
    }
  }
}

// Réponse
{
  "job_id": "J-1.1",
  "passed": true,
  "approved": true,
  "action": "commit",  // ou "retry" ou "escalate"
  "audit": {...}
}
```

### 5.3 Insights

#### `harvest_insights`
Extrait patterns et leçons d'un thème complété (à appeler après que tous les jobs d'un thème sont en `commit`).

```json
// Appel
{
  "method": "tools/call",
  "params": {
    "name": "harvest_insights",
    "arguments": {
      "theme_id": "theme-1",
      "theme_name": "Extract auth-service",
      "jobs_summary": "3 jobs commit, 1 retry, 0 escalations",
      "metrics": {"coverage": 87, "tokens_used": 4500}
    }
  }
}

// Réponse
{
  "insights": [
    {
      "type": "pattern",
      "description": "Migration de schéma réussit toujours en 2 étapes (backfill + swap)"
    },
    {
      "type": "lesson",
      "description": "Toujours valider la signature HMAC avant d'évaluer les guardrails (couche 1)"
    }
  ]
}
```

### 5.4 Worker lifecycle

#### `check_jobs_status`
Status d'un projet : themes pending/completed/failed, basé sur inférence des commits WAL.

```json
// Appel
{"method": "tools/call", "params": {"name": "check_jobs_status", "arguments": {"project_id": "..."}}}

// Réponse
{
  "project_id": "project-...",
  "status": "in_progress",
  "themes": [
    {"theme_id": "theme-1", "status": "completed", "jobs_count": 3},
    {"theme_id": "theme-2", "status": "in_progress", "jobs_count": 1}
  ],
  "total_commits": 47,
  "pending_jobs": 5,
  "completed_jobs": 12
}
```

#### `rollback`
Rollback à un commit précédent. Utile si un job s'est terminé avec un état incohérent.

```json
// Appel
{
  "method": "tools/call",
  "params": {
    "name": "rollback",
    "arguments": {
      "project_id": "project-...",
      "target_commit_id": null,  // null = commit précédent
      "reason": "État incohérent après sync_reflect"
    }
  }
}
```

#### `abort`
Emergency stop d'un projet. Tous les dispatchs ultérieurs sont bloqués.

```json
// Appel
{"method": "tools/call", "params": {"name": "abort", "arguments": {"project_id": "...", "reason": "User requested"}}}
```

#### `recover_project`
**Crash recovery** : liste les entries WAL non-committées et applique les politiques par type d'action :
- `abort`, `rollback`, `approval_received` → **commit** (actions terminales)
- `sync_reflect`, `task_completed` → **escalate** (laisser uncommitted, intervention humaine)
- reste → **rolled back** (marqué dans WAL)

```json
// Appel
{"method": "tools/call", "params": {"name": "recover_project", "arguments": {"project_id": "..."}}}

// Réponse
{
  "project_id": "project-...",
  "uncommitted_count": 3,
  "rolled_back": ["entry-id-1"],
  "escalated": ["entry-id-2", "entry-id-3"]
}
```

### 5.5 Observabilité

#### `get_metrics`
Retourne les compteurs **Prometheus** au format text/plain.

```json
// Appel
{"method": "tools/call", "params": {"name": "get_metrics", "arguments": {}}}

// Réponse
# HELP cortex_jobs_dispatched_total Total jobs dispatched
# TYPE cortex_jobs_dispatched_total counter
cortex_jobs_dispatched_total 12
# HELP cortex_jobs_approved_total Total jobs approved
# TYPE cortex_jobs_approved_total counter
cortex_jobs_approved_total 9
...
cortex_uptime_seconds 3600
```

**Séries exposées (13)** :
| Métrique | Type | Description |
|---|---|---|
| `cortex_jobs_dispatched_total` | counter | Jobs dispatchés (un par task du plan) |
| `cortex_jobs_approved_total` | counter | Jobs validés par Red-Team |
| `cortex_jobs_rejected_total` | counter | Jobs en retry |
| `cortex_jobs_escalated_total` | counter | Jobs escaladés humainement |
| `cortex_red_team_blocks_total` | counter | Audits Red-Team qui ont bloqué (passed=false) |
| `cortex_pre_mortem_guards_emitted_total` | counter | Guardrails émis |
| `cortex_hmac_verifications_total{result="ok\|fail"}` | counter | Vérifications HMAC |
| `cortex_recovery_rolled_back_total` | counter | Entries rolled back par recovery |
| `cortex_recovery_escalated_total` | counter | Entries escalated par recovery |
| `cortex_llm_requests_total` | counter | Appels LLM totaux |
| `cortex_llm_tokens_consumed_total` | counter | Tokens LLM consommés |
| `cortex_uptime_seconds` | gauge | Uptime du serveur en secondes |

**Intégration Prometheus** : scrape le serveur Cortex via un sidecar qui interroge l'outil MCP `get_metrics` (ex : via `mcp-exporter`).

---

## 6. Workflow end-to-end

```
User: "Refactorise le module auth"
        │
        ▼
Hermes ──── intercept_plan ────▶ Cortex
                                   │
                                   ├── Architect génère FractalPlan
                                   ├── Cost-gating summary
                                   ├── 3 themes, criticité 4
                                   ├── WAL commit (snapshot)
                                   └── Retourne plan + requires_user_approval
        ◀────────────────────
        │
        ▼
User: "J'approuve"
        │
        ▼
Hermes ──── approve_and_execute ─▶ Cortex
                                    │
                                    ├── Kahn topological sort → 2 phases
                                    ├── DispatchOrder (phases parallélisables)
                                    └── Log approval dans WAL
        ◀─────────────────────
        │
        ▼
Phase 1 : theme-1 + theme-2 (parallèle)
        │
        ├── theme-1: criticité 4 → pre_mortem
        │                Cortex ─▶ 5 guardrails + HMAC signature
        │                Worker exécute + appelle sync_reflect à la fin
        │                Cortex ─▶ red_team_audit (5 couches)
        │                       │
        │                       ├── pass → commit
        │                       └── fail → retry (jusqu'à 3x) ou escalate
        │
        └── theme-2: criticité 2 → pas de pre_mortem
                        Worker exécute + sync_reflect → commit
        │
        ▼
Phase 2 : theme-3 (dépend de theme-1)
        │
        ▼
Tous themes commités → harvest_insights
        │
        ▼
Cortex: 5 patterns, 3 lessons extraits
```

---

## 7. Anti-tampering HMAC

Le workflow HMAC empêche les workers de modifier les guardrails :

```
Cortex pre_mortem:
  guardrails = [...]
  signature = HMAC-SHA256(secret, {job_id, guardrails, dod})
  
Hermes passe au worker: {guardrails, signature}

Worker peut lire les guardrails mais ne peut pas les modifier :
  tampered = modifier(guardrails)  // ex: enlever le check de backup
  signature' = HMAC-SHA256(secret, {job_id, tampered, dod})  // ≠ signature
  
Cortex red_team_audit:
  payload = {job_id, guardrails, dod}
  if HMAC-SHA256(secret, payload) ≠ signature:
    fail loud "guardrails may have been tampered with"
    inc_hmac_fail
    return error
```

**Note** : si `CORTEX_HMAC_SECRET` n'est pas configuré, la signature est désactivée (mode dégradé, **non recommandé en production**).

---

## 8. Crash recovery

Si le serveur Cortex plante (kill -9, OOM, machine reboot) pendant un commit, les entries WAL sont dans un état inconsistant : certaines sont `write_prepare` mais pas `write_commit`.

Au redémarrage, **appeler** `recover_project` :
```json
{"method": "tools/call", "params": {"name": "recover_project", "arguments": {"project_id": "project-..."}}}
```

Cortex lit `wal_entries` filtré par `committed=0`, applique les politiques :
- **Politiques par action** :
  - `approval_received`, `abort`, `rollback` → **commit** (safe à finaliser)
  - `sync_reflect`, `task_completed` → **escalate** (laisser uncommitted, intervention humaine)
  - reste → **rolled back** (marqué `recovery_rollback` dans le WAL)

**Idéalement** : un cron appelle `recover_project` pour tous les projets actifs au boot du serveur.

---

## 9. Troubleshooting

### Cortex ne démarre pas
**Erreur** : `CORTEX_LLM_API_KEY required for provider=openai`
**Fix** : Vérifier que la variable d'environnement est bien set :
```bash
echo $CORTEX_LLM_API_KEY  # doit afficher la clé
```

### Tests qui failent au premier run
**Erreur** : `WAL database is locked`
**Fix** : Vérifier qu'aucune autre instance de `cortex-mcp` ne tourne :
```bash
ps aux | grep cortex-mcp   # Linux/Mac
tasklist | findstr cortex  # Windows
```

### Les métriques sont à 0
**Cause** : le tool `get_metrics` n'a jamais été appelé depuis le boot. C'est normal : les compteurs sont incrémentés uniquement lors d'appels d'outils.
**Fix** : faire un appel `intercept_plan` puis `get_metrics` pour vérifier.

### LLM timeout
**Erreur** : `LLM call failed: timeout`
**Fix** : augmenter `CORTEX_LLM_TIMEOUT` (default 60s) :
```bash
export CORTEX_LLM_TIMEOUT=120
```

### HMAC verification failed mais guardrails identiques
**Cause** : la signature a été forwardée avec un `job_id` ou `definition_of_done` différent de ceux utilisés à la signature.
**Fix** : vérifier que `sync_reflect.arguments` contient bien les **mêmes** `job_id` et `definition_of_done` que `pre_mortem.arguments`.

### SQLite WAL mode désactivé
**Erreur** : `database is locked` sur accès concurrent
**Fix** : `CORTEX_WAL_URL` doit finir par `?mode=rwc` ET utiliser un **fichier** (pas `memory:`).

---

## 10. Architecture interne (résumé)

```
cortex-mcp/
├── crates/
│   ├── cortex-core/          # Domain model, WAL, metrics, HMAC primitives
│   │   ├── wal.rs            # WalService (SQLite WAL, RecoveryReport)
│   │   ├── routing.rs        # RoutingRules
│   │   ├── metrics.rs        # Metrics (Prometheus counters, 4 tests)
│   │   └── ...
│   ├── cortex-actors/        # Actor Model (DashMap registry)
│   ├── cortex-brains/        # 4 brains : Architect, PreMortem, RedTeam, Insights
│   │   ├── llm_client.rs     # LlmClient trait + MockLlmClient
│   │   ├── http_llm_client.rs # HttpLlmClient (OpenAI-compat, reasoning)
│   │   ├── architect.rs      # FractalPlan generator
│   │   ├── paranoiac.rs      # Pre-Mortem guardrails (HMAC signed)
│   │   ├── red_team.rs       # 5-layer audit + HMAC verify
│   │   ├── insights.rs       # Pattern extraction
│   │   └── cost_gating.rs    # Cost analysis
│   ├── cortex-security/      # HMAC-SHA256 sign/verify
│   └── cortex-mcp-server/    # MCP server (JSON-RPC 2.0 stdio)
│       ├── main.rs           # env config + stdio loop
│       ├── protocol.rs       # JSON-RPC 2.0 types
│       ├── dispatch.rs       # tool routing (12 tools)
│       ├── server.rs         # CortexServer<C: LlmClient + Clone> + handlers
│       └── config.rs         # config loader
└── target/release/
    └── cortex-mcp.exe        # Binaire (7 MB Windows)
```

**Couches** :
1. **Domain (cortex-core)** : WAL, modèle, métriques, primitives HMAC
2. **Brains (cortex-brains)** : logique pure des 4 brains, sans état persistant
3. **Security (cortex-security)** : sign/verify HMAC-SHA256
4. **Server (cortex-mcp-server)** : orchestration, handlers MCP, instrumentation

---

## 11. Limites connues

- **Historique LLM** : Cortex n'envoie pas l'historique de conversation à l'LLM. Chaque appel est stateless. Le contexte est porté par le `Scratchpad` et passé explicitement.
- **Backend WAL** : Le **binaire** `cortex-mcp` est compilé en mode SQLite par défaut. Le **support PostgreSQL** est disponible dans la **lib `cortex-core`** via la feature `--features cortex-core/postgres` (build : `cargo build --no-default-features --features cortex-core/postgres`). Les autres crates (cortex-actors, cortex-brains, cortex-mcp-server) peuvent être migrées dans une session dédiée.
- **Concurrence** : SQLite WAL supporte plusieurs readers + 1 writer. Pour >1 writer simultané ou scale multi-host, migrer vers PostgreSQL (qui supporte JSONB natif, jusqu'à 100+ connexions concurrentes).
- **Métriques** : 13 compteurs lock-free + 8 histogrammes (Session 6). Compatible Prometheus.
- **Transport MCP** : `stdio` uniquement (pas de `sse` / `http`). Si besoin d'un transport réseau, reverse-proxy via `mcp-proxy`.

### Backends WAL supportés (Session 6 option E)

| Backend | Activation | Cas d'usage | Schéma JSON |
|---|---|---|---|
| **SQLite** (défaut) | aucune feature | Local, tests, mode embedded, single-writer | `TEXT` |
| **PostgreSQL** | `--features cortex-core/postgres` | Prod multi-host, haute concurrence, scale | `JSONB` (natif) |

Pour activer PostgreSQL dans une lib downstream :
```toml
# Cargo.toml
cortex-core = { path = "../cortex-core", default-features = false, features = ["postgres"] }
```

Puis : `CORTEX_WAL_URL=postgres://user:pass@host:5432/cortex_db`

---

## 12. Tests & qualité

```bash
# Tests workspace
cd ~/Documents/Cortex/cortex-mcp
cargo test --workspace

# Couverture (~64% actuellement)
cargo tarpaulin --workspace --out Html
```

**Statistiques Session 5** :
- 135 tests passing (3 cortex-actors, 65 cortex-brains, 33 cortex-core, 30 cortex-mcp-server, 4 cortex-security)
- 12 outils MCP
- 5 brains + 4 policies de recovery
- HMAC + Prometheus + crash recovery intégrés

---

## 13. Roadmap post-MVP

- [ ] Histogrammes Prometheus (latence LLM, durée dispatch)
- [ ] Web UI standalone (Tauri/egui) pour visualisation plans
- [ ] Support PostgreSQL WAL (au lieu de SQLite)
- [ ] Multi-project concurrent dispatch
- [ ] Plugin LLM custom (Lua/WASM)
- [ ] Webhook sur événements critiques (escalation, abort)

---

## Annexe A — Schéma de la base SQLite

```sql
-- Migrations appliquées au boot si absentes
CREATE TABLE IF NOT EXISTS wal_entries (
    entry_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    timestamp INTEGER NOT NULL,  -- millis since epoch
    action TEXT NOT NULL,
    job_id TEXT,
    theme_id TEXT,
    data_json TEXT NOT NULL,
    committed INTEGER NOT NULL DEFAULT 0  -- 0=prepared, 1=committed
);
CREATE INDEX IF NOT EXISTS idx_wal_project_committed ON wal_entries(project_id, committed);

CREATE TABLE IF NOT EXISTS wal_commits (
    commit_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    timestamp INTEGER NOT NULL,
    previous_commit TEXT,
    mutation_type TEXT NOT NULL,
    trigger_reason TEXT,
    diff_json TEXT NOT NULL,
    snapshot_json TEXT NOT NULL,
    checksum TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS project_states (
    project_id TEXT PRIMARY KEY,
    state_json TEXT NOT NULL,  -- e.g. {"plan": {...}, "status": "..."}
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS scratchpad_snapshots (
    snapshot_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    timestamp INTEGER NOT NULL,
    content_json TEXT NOT NULL
);
```

## Annexe B — Catalogue d'échecs F1-F19

Le `pre_mortem` référence ces modes d'échec (cf. `failure-recovery-system.md`) :

| Code | Description |
|---|---|
| F1 | Fichier manquant |
| F2 | Permissions insuffisantes |
| F3 | Migration DB échouée |
| F4 | Tests qui échouent |
| F5 | Build cassé |
| F6 | Dépendance circulaire |
| F7 | Backup absent |
| F8 | Race condition |
| F9 | Token API expiré |
| F10 | Schema drift |
| F11 | Timeout réseau |
| F12 | Disk plein |
| F13 | Memory leak |
| F14 | Encoding cassé |
| F15 | Connexion DB pool exhausted |
| F16 | Index manquant |
| F17 | SSL cert invalide |
| F18 | Worker orphelin |
| F19 | Token LLM quota dépassé |

---

**Version** : 0.1.0 (MVP Session 5)  
**Date** : 2026-06-04  
**Auteur** : Cortex team (développé via Hermes Agent)
