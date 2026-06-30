# nullrouter

Pure Rust port of [decolua/9router](https://github.com/decolua/9router) (pinned at commit `0b3c794`, v0.5.15).

**Status**: Phase 0 (workspace skeleton) — in progress.

## Architecture

- **Pure Rust** — zero Node.js at runtime or build time
- **Single static binary** — all-in-one deployment
- **Bug-for-bug parity** — 25 translator fixtures are the byte-identical oracle
- **Tech stack**:
  - Web server: Actix-web 4.14 + leptos_actix 0.7 (SSR + WASM hydrate)
  - MITM proxy: Pingora 0.4
  - Database: rusqlite 0.32 (bundled, sync API matching better-sqlite3)
  - Dashboard: Leptos 0.7 (SSR + client hydration)

## Workspace Layout

```
crates/
├── nullrouter-core       # Provider registry, config, shared types
├── nullrouter-sse        # SSE pipeline (translator, executors, handlers)
├── nullrouter-db         # SQLite storage layer
├── nullrouter-auth       # JWT, bcrypt, API keys
├── nullrouter-api        # HTTP route handlers (125 routes)
├── nullrouter-mitm       # MITM proxy with cert minting
├── nullrouter-dashboard  # Leptos UI (SSR + WASM)
├── nullrouter-server     # Main binary (Actix + Pingora supervisor)
└── nullrouter-cli        # Setup/admin CLI
```

## Development

**Requirements**: Rust 1.88+, cmake, build-essential, pkg-config

```bash
# Check workspace
cargo check --workspace

# Build
cargo build --workspace

# Run tests
cargo test --workspace

# Format
cargo fmt --all

# Lint
cargo clippy --workspace -- -D warnings
```

## Upstream Reference

`inspire/` directory (read-only, never modified) pins the upstream source at commit `0b3c794` (v0.5.15, 2026-06-29). All porting decisions reference this snapshot.

## License

MIT (matching upstream)
