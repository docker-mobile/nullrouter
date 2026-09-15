# 🚀 Getting Started with NullRouter

This guide walks you through installing, configuring, running, and verifying NullRouter on Linux, macOS, and Windows (WSL2).

---

## 📋 System Requirements

- **Operating System**: Linux (x86_64, aarch64), macOS (Apple Silicon / Intel), Windows 10/11 via WSL2.
- **Memory**: Minimum 64 MB RAM available (NullRouter typically consumes ~18 MB).
- **Disk Space**: ~250 MB for compiled release binary and state.
- **Network**: Port `20128` free for the Pingora gateway.

---

## 📦 Installation Methods

### Method 1: Build from Source (Cargo)

If you have the Rust toolchain installed (edition 2024 / Rust 1.85+):

```bash
# Clone the repository
git clone https://github.com/nullrouter/nullrouter.git
cd nullrouter

# Build the release profile
cargo build --release

# Run the unified launcher
cargo run --release
```

The release binary will be placed at `target/release/nullrouter-gateway`.

### Method 2: Shell Launch Script

The repository includes a self-bootstrapping run script that validates ports, checks dependencies, builds the Leptos WebAssembly UI, and starts all services in proper dependency order:

```bash
chmod +x ./run.sh
./run.sh
```

Flags supported by `./run.sh`:
- `--dev`: Start in debug mode with verbose logging (`RUST_LOG=debug`).
- `--port <PORT>`: Override the default port `20128`.
- `--no-ui`: Run in headless daemon mode without serving the Leptos WASM assets.

### Method 3: Docker & Docker Compose

For containerized deployment on servers or NAS devices:

```yaml
# docker-compose.yml
version: '3.8'

services:
  nullrouter:
    image: ghcr.io/nullrouter/nullrouter:latest
    container_name: nullrouter
    restart: unless-stopped
    ports:
      - "20128:20128"
    volumes:
      - nullrouter-data:/root/.nullrouter
    environment:
      - RUST_LOG=info
      - NULLROUTER_HOST=0.0.0.0
      - NULLROUTER_PORT=20128

volumes:
  nullrouter-data:
```

Run with:
```bash
docker compose up -d
```

### Method 4: systemd Service (Linux Production)

To run NullRouter as a system daemon that starts on boot:

1. Copy the compiled binary to `/usr/local/bin/nullrouter`:
   ```bash
   sudo cp target/release/nullrouter-gateway /usr/local/bin/nullrouter
   ```

2. Create the systemd service definition at `/etc/systemd/system/nullrouter.service`:
   ```ini
   [Unit]
   Description=NullRouter AI Gateway & Token Optimizer
   After=network.target

   [Service]
   Type=simple
   User=root
   WorkingDirectory=/root
   ExecStart=/usr/local/bin/nullrouter
   Restart=always
   RestartSec=3
   LimitNOFILE=65535
   Environment=RUST_LOG=info

   [Install]
   WantedBy=multi-user.target
   ```

3. Enable and start:
   ```bash
   sudo systemctl daemon-reload
   sudo systemctl enable --now nullrouter
   sudo systemctl status nullrouter
   ```

---

## ⚙️ Initial Configuration

Upon first boot, NullRouter initializes its state store in `~/.nullrouter/nullrouter-state.json` (or uses the local `./nullrouter-state.json` if run locally).

### Verifying Gateway Health

Verify that the Pingora gateway is answering HTTP queries:

```bash
curl -I http://localhost:20128/api/health
```

Expected output:
```http
HTTP/1.1 200 OK
content-type: application/json
content-length: 15
date: Mon, 14 Sep 2026 20:45:00 GMT
server: pingora

{"status":"ok"}
```

---

## 🖥️ Accessing the Dashboard

Open your browser and navigate to:
```
http://localhost:20128/dashboard
```

Key Dashboard sections:
- **Overview**: Real-time request volume, active routing tier, token compression metrics.
- **Providers**: Add and manage API keys for 40+ providers.
- **Combos**: Define smart fallback chains (e.g., `smart = claude-sonnet-4-6 -> deepseek-v4-pro -> kiro` per `models.dev`).
- **Chat Playground**: Test models interactively with live streaming and collapsible `<think>` reasoning chains.
- **Settings**: Adjust RTK token compression strength, rate limits, and network timeouts.

Next: Check out **[Client Integrations](Client-Integrations.md)** to connect your favorite editor!
