# Cortex MCP Server

> Strategic brain for Hermes Agent - Plan, orchestrate, and supervise complex projects

## 🚧 Status: Early Development (Phase 4 - MVP)

This project is under active development. See `planning/todo-phase4.md` for roadmap.

## 📦 Project Structure

```
cortex-mcp/
├── crates/
│   ├── cortex-core/          # Data structures + WAL persistence
│   ├── cortex-actors/        # Actor Model implementation (tokio)
│   ├── cortex-brains/        # LLM prompts (Architect, Pre-Mortem, Red-Team)
│   ├── cortex-security/      # HMAC validation
│   └── cortex-mcp-server/    # Main MCP server (stdio)
└── tests/                    # Integration tests
```

## 🔧 MCP Tools (7 planned)

| Tool | Status | Description |
|------|--------|-------------|
| `get_routing_rules` | 🟡 WIP | Boot handshake for Hermes |
| `intercept_plan` | 🟡 WIP | Generate fractal plan |
| `approve_and_execute` | 🟡 WIP | Launch workers (Fire-and-Forget) |
| `sync_reflect` | 🟡 WIP | Validate worker output |
| `check_jobs_status` | ⏸️ Planned | Query project status |
| `harvest_insights` | ⏸️ Planned | Extract patterns + lessons |
| `rollback` | ⏸️ Planned | Restore to previous commit |
| `abort` | ⏸️ Planned | Emergency stop |

## 🏗️ Building

```bash
# Build all crates
cargo build

# Run tests
cargo test

# Check workspace
cargo check --workspace
```

## 🧪 Testing

```bash
# Unit tests
cargo test --lib

# Integration tests
cargo test --test '*'

# Property tests (proptest)
cargo test --test property

# Coverage (requires cargo-tarpaulin)
cargo tarpaulin --workspace --out Html
```

## 📚 Documentation

See `/planning/` for design documents:
- `specification-cortex-v2.md` - Complete specification
- `simulation-workflow.md` - Visual workflow simulation
- `delegate_task-cortex-integration.md` - delegate_task integration patterns
- `todo-phase4.md` - Implementation roadmap

## 📄 License

MIT
