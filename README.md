# 🧠 Cortex MCP

> Strategic brain for [Hermes Agent](https://github.com/gaulet/hermes) — saves tokens, prevents failures, and recovers from crashes.

[![Tests](https://github.com/gaulet/cortex/actions/workflows/cortex-tests.yml/badge.svg)](https://github.com/gaulet/cortex/actions)
[![Postgres integration](https://github.com/gaulet/cortex/actions/workflows/cortex-postgres-integration.yml/badge.svg)](https://github.com/gaulet/cortex/actions)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-stable-orange.svg)](https://www.rust-lang.org)
[![MCP](https://img.shields.io/badge/MCP-stdio-green.svg)](https://modelcontextprotocol.io)

**Cortex** is a Rust MCP (Model Context Protocol) server that acts as a **strategic brain** for AI agents like Hermes. It intercepts complex tasks, decomposes them into themes, runs pre-mortem analysis and red-team audits, and persists state in a Write-Ahead Log for crash recovery.

## ⚡ Quick start

```bash
# 1. Build (SQLite, default)
cargo build --release -p cortex-mcp-server

# 2. Run (mock LLM, in-memory DB)
./target/release/cortex-mcp
# Server listens on stdio, JSON-RPC 2.0

# 3. Or with a real LLM (OpenAI-compatible)
export CORTEX_LLM_PROVIDER=openai
export CORTEX_LLM_API_KEY=sk-...
export CORTEX_LLM_BASE_URL=https://openrouter.ai/api/v1
export CORTEX_LLM_MODEL=anthropic/claude-sonnet-4
export CORTEX_LLM_REASONING=max
export CORTEX_WAL_URL="sqlite://$HOME/.local/share/cortex/cortex.db"
./target/release/cortex-mcp
```

See [CORTEX-USER-GUIDE.md](CORTEX-USER-GUIDE.md) for the full guide, and [DEPLOY.md](DEPLOY.md) for production deployment (Docker, Postgres, monitoring).

## 🎯 What Cortex does

| Tool | Purpose |
|------|---------|
| `intercept_plan` | Decompose an intent into themes, estimate cost, identify risks |
| `pre_mortem` | Generate guardrails (anti-patterns) for high-criticity themes |
| `red_team_audit` | 5-layer audit: HMAC, DoD, guardrails, convergence, edge cases |
| `harvest_insights` | Extract patterns from past commits |
| `approve_and_execute` | Approve a plan and log it to WAL |
| `sync_reflect` | Reflect on completed work, decide commit/retry/escalate |
| `check_jobs_status` | Aggregate status across all themes |
| `rollback` | Roll back to a previous commit |
| `abort` | Emergency stop — mark project aborted |
| `recover_project` | Replay uncommitted WAL entries after crash |
| `get_metrics` | Prometheus metrics (13 counters + 1 gauge + 8 histograms) |
| `get_routing_rules` | Introspection on routing config |

**12 MCP tools** total, all via stdio JSON-RPC 2.0.

## Status

- **30+ commits** on `master`
- **166/166 tests passing** (SQLite default + 4 PG live tests via `TEST_POSTGRES_URL`)
- **0 warnings clippy**
- **0 panics en code prod** (audit Session 7)
- **6 crates** workspace (core, actors, brains, security, webhooks, server)
- **12 MCP tools** + **5 webhook events** + **13 compteurs + 1 gauge + 8 histogrammes** Prometheus
- **Dual backend WAL** : SQLite (défaut) + PostgreSQL (opt-in via `--features cortex-core/postgres`)

## 🏗️ Architecture

5+1 Rust crates, **zero external runtime deps**:

```
cortex-mcp-server (MCP stdio, 12 tools)
    ↓
cortex-actors (Actor Model, 1 actor = 1 project)
    ↓
cortex-brains (Architect, PreMortem, RedTeam, InsightsHarvester)
    ↓
cortex-webhooks (5 events HTTP sortants : fire-and-forget + retry)
    ↓
cortex-core (WAL dual backend SQLite/PG, Metrics, Routing, Scratchpad)
    ↓
cortex-security (HMAC-SHA256 anti-tampering)
    ↓
cortex-webhooks (HTTP notifications on critical events)
```

- **Async**: `tokio` (Actor Model natif, pas de `Arc<Mutex<T>>`)
- **Transport**: `stdio` JSON-RPC 2.0 "from scratch" (~350 LoC)
- **DB**: SQLite (default) or PostgreSQL (`--features cortex-core/postgres`)
- **No native deps**: compiles anywhere Rust does

## 📊 Status

- **27 commits** on master
- **166 unit/integration tests** (SQLite) + **4 live PG tests** (PostgreSQL)
- **12 MCP tools** exposed
- **Cross-platform**: Windows, Mac, Linux, WSL2, Docker

## 🚀 Deployment

```bash
# Docker Compose (Postgres + cortex)
cd ~/Documents/Cortex/cortex-mcp
docker compose up -d

# Or compile with Postgres backend
cargo build --release -p cortex-mcp-server --no-default-features --features cortex-core/postgres
export CORTEX_WAL_URL=postgres://cortex:***@localhost:5432/cortex_db
```

See [DEPLOY.md](DEPLOY.md) for the full production guide (10 sections, including monitoring queries and backup strategies).

## 🧪 Testing

```bash
# All tests (SQLite)
cargo test --workspace

# Live Postgres tests (requires `docker compose up -d`)
TEST_POSTGRES_URL=postgres://cortex:***@localhost:5432/cortex_db \
  cargo test -p cortex-core --features postgres --lib --tests -- --include-ignored
```

## 🔌 Integration with Hermes

Cortex is designed to be plugged into any MCP-compatible agent (Hermes, Claude Desktop, etc.). See the [`hermes-config-cortex-snippet.yaml`](planning/hermes-config-cortex-snippet.yaml) for a Hermes config example.

## 📚 Documentation

| Doc | What it covers |
|-----|----------------|
| [CORTEX-USER-GUIDE.md](CORTEX-USER-GUIDE.md) | End-to-end guide (config, tools, troubleshooting, webhooks) |
| [DEPLOY.md](DEPLOY.md) | Production deployment (Docker, Postgres, monitoring) |
| [planning/carte-mentale-workflow.md](planning/carte-mentale-workflow.md) | Pipeline diagram of the 12 tools |
| [planning/carte-mentale-database.md](planning/carte-mentale-database.md) | Storage layers + recovery policies |
| [planning/carte-fichiers-architecture.md](planning/carte-fichiers-architecture.md) | Crate structure + file index |
| [planning/direction-objectif.md](planning/direction-objectif.md) | **Anchoring doc**: 12 invariants, 10 anti-patterns |

## 🤝 Contributing

Cortex is currently in **Session 6** of development. The architecture is stable (Actor Model, WAL, HMAC) and we are now adding:
- Webhooks (✅ done, 5 events)
- PostgreSQL support (✅ done, feature-gated)
- Prometheus histograms (✅ done, 8 histograms)

PRs welcome on the `master` branch. Please read `planning/direction-objectif.md` first.

## 📜 License

MIT
