# ⚡ Welcome to the NullRouter Wiki

**NullRouter** is the ultra-high-performance, zero-copy AI router, reverse proxy, and token optimizer specifically built for modern agentic coding workflows (Claude Code, Cursor, Cline, Roo Code, OpenAI Codex, Continue, Windsurf, Aider).

Engineered entirely in **Rust (2024 edition)** and powered by **Cloudflare's Pingora** proxy engine, NullRouter delivers sub-50µs routing overhead, native hybrid extended thinking normalization, zero-copy SSE streaming, and 20–40% token savings through its RTK (Runtime Token-Kompact) engine.

---

## 📑 Wiki Table of Contents

### 🚀 Getting Started & Architecture
1. **[Getting Started & Installation](Getting-Started.md)** — Step-by-step setup via Cargo, Docker, systemd, and single-command scripts.
2. **[Architecture & Performance](Architecture-and-Performance.md)** — Deep dive into Pingora, Actix microservices, zero-copy buffers, and benchmark comparisons.

### 🔌 Integrations & Configurations
3. **[Client Integrations](Client-Integrations.md)** — One-click setups for Claude Code, Cursor, Cline, Roo Code, Codex, Continue, Windsurf, Aider, and Neovim.
4. **[Provider Configuration](Provider-Configuration.md)** — Complete configuration guides for 40+ providers (Subscription, Cheap, Free, Local Ollama/vLLM).
5. **[Smart Routing & Failover](Smart-Routing-and-Failover.md)** — Tiered routing rules, auto-failover, model aliases (`auto`, `fast`, `coder`), and multi-account load balancing.
6. **[RTK Token Saver](RTK-Token-Saver.md)** — Understanding Runtime Token-Kompact: how it shrinks diffs, grep logs, and tool results by 20–40%.

### 🖥️ Dashboard & APIs
7. **[Playground & WebAssembly Dashboard](Playground-and-WebAssembly-Dashboard.md)** — Leptos 0.7 WASM UI, 35 localized languages, and interactive `<think>` reasoning chain visualizer.
8. **[API Reference](API-Reference.md)** — Comprehensive REST and SSE documentation for `/v1/chat/completions`, `/v1/messages`, and management endpoints.
9. **[Security & Privacy](Security-and-Privacy.md)** — Local credential encryption, zero telemetry, secret masking, and TLS termination.

### 🩺 Diagnostics & Troubleshooting (Exhaustive)
10. **[Diagnostics & Troubleshooting Playbook](Diagnostics-and-Troubleshooting.md)** — Complete error code reference (`E1001`–`E4003`), port collision checks, proxy profiling, socket exhaustion, and step-by-step recovery.
11. **Localized Diagnostic Guides**:
    - [🇨🇳 中文系统诊断指南](i18n-Diagnostics-Guide-zh.md)
    - [🇪🇸 Guía de Diagnóstico en Español](i18n-Diagnostics-Guide-es.md)
    - [🇯🇵 日本語診断ガイド](i18n-Diagnostics-Guide-ja.md)
    - [🇧🇷 Guia de Diagnóstico em Português](i18n-Diagnostics-Guide-pt.md)
    - [🇻🇳 Hướng dẫn chẩn đoán Tiếng Việt](i18n-Diagnostics-Guide-vi.md)

---

## ⚡ Quick System Status Check

To quickly verify your local NullRouter instance:

```bash
# Gateway Health Check
curl -I http://localhost:20128/api/health

# Verify Loaded Provider Count
curl -s http://localhost:20128/api/state | jq '.connections | length'

# Test Chat Completion
curl -s http://localhost:20128/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"auto","messages":[{"role":"user","content":"ping"}]}' | jq .
```
