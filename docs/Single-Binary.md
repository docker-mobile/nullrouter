# Single-Binary Mode

NullRouter can run as a single binary that forks all 7 microservices as child processes.

## Quick Start

```bash
# Build all binaries (release)
cargo build --release

# Run the supervisor
./target/release/nullrouter

# Or with debug binaries
./target/release/nullrouter --debug
```

## How It Works

1. The supervisor generates a random 6-digit runtime directory: `/tmp/nullrouter.{random6}`
2. It spawns each microservice as a child process with the correct env vars
3. All services use `127.0.0.1` and ports derived from the base port (default: 20128)
4. Ctrl-C sends SIGINT to the entire process group, shutting down all services
5. The runtime directory is cleaned up on exit

## Port Allocation

| Service | Default Port | Env Var |
|---|---|---|
| Gateway | 20128 | `NULLROUTER_BASE_PORT` |
| API | 20129 | (derived) |
| Dashboard Host | 20130 | (derived) |
| Catalog | 20131 | (derived) |
| Runtime | 20132 | (derived) |
| Events | 20133 | (derived) |
| State | 20134 | (derived) |
| Auth | 20135 | (derived) |

## Environment Variables

| Variable | Default | Description |
|---|---|---|
| `NULLROUTER_BASE_PORT` | `20128` | Base port; all others derived |
| `NULLROUTER_STATE_FILE` | `/tmp/nullrouter.{rand}/nullrouter-state.json` | State file path |
| `NULLROUTER_VERBOSE` | `false` | Enable info-level logging |
| `RUST_LOG` | `warn` | Logging level (overridden by `--verbose`) |
