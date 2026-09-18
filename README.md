<div align="center">

# ⚡ NullRouter

**Ultra-High-Performance, Zero-Copy AI Router & Token Optimizer for Agentic Coding**

[![Rust](https://img.shields.io/badge/rust-edition%202024-orange.svg?logo=rust)](https://www.rust-lang.org)
[![Pingora](https://img.shields.io/badge/powered%20by-Cloudflare%20Pingora-blue.svg?logo=cloudflare)](https://github.com/cloudflare/pingora)
[![License: EPL-2.0](https://img.shields.io/badge/License-EPL--2.0-blue.svg)](LICENSE)
[![Latency](https://img.shields.io/badge/dispatch%20latency-%3C50%C2%B5s-brightgreen.svg)](https://github.com/nullrouter/nullrouter)
[![WebAssembly](https://img.shields.io/badge/UI-Leptos%20WASM-purple.svg?logo=webassembly)](apps/dashboard-leptos)
[![Providers](https://img.shields.io/badge/providers-352%20integrated-success.svg)](wiki/Provider-Configuration)

<p align="center">
  <b>Never drop an agentic session. Cut token usage by 15–95% with NullStack (RTK+Caveman). Route intelligently across 352 providers with sub-millisecond dispatch.</b>
</p>

[🚀 Quick Start](#-quick-start) • [✨ Key Features](#-key-features) • [🔌 Client Integrations](#-client-integrations) • [🌐 Providers](#-supported-providers) • [📊 Benchmarks](#-benchmarks) • [📖 Documentation Wiki](wiki/Home.md) • [🩺 Diagnostics](wiki/Diagnostics-and-Troubleshooting.md)

---

### 🌐 Read this in other languages

[English](README.md) • [Español](i18n/README.es.md) • [简体中文](i18n/README.zh-CN.md) • [日本語](i18n/README.ja-JP.md) • [Português (Brasil)](i18n/README.pt-BR.md) • [Français](i18n/README.fr.md) • [Deutsch](i18n/README.de.md) • [Русский](i18n/README.ru.md) • [한국어](i18n/README.ko.md) • [Tiếng Việt](i18n/README.vi.md)

</div>

---

## 💡 Why NullRouter?

Modern agentic coding workflows (Claude Code, Cursor, Cline, Roo Code, Codex) run massive prompt loops. Standard routers built on Node.js or Python introduce heavy garbage collection pauses, memory bloat, high latency, and lack protocol-level reasoning chain normalization.

**NullRouter is engineered from the ground up in Rust using Cloudflare's Pingora engine to provide:**

| Feature | NullRouter (Rust + Pingora) | Traditional Node.js Routers |
| :--- | :--- | :--- |
| **Proxy Engine** | Cloudflare Pingora (zero-copy async HTTP) | Express / Koa / Fastify |
| **Internal Routing Latency** | **< 50 microseconds** fast path | 15 – 45 milliseconds |
| **Memory Footprint** | **~18 MB** steady state | 120 – 350 MB |
| **SSE Streaming Allocation** | Direct buffer writer (`0` intermediate allocations) | Repeated string concatenation & JSON parse loops |
| **NullStack Token Saver** | Rust RTK (49 filters) + Caveman stacked, 15–95% (`crates/rtk`, `crates/caveman`) | Naive regex or uncompressed tool payloads |
| **Reasoning Normalization** | Full hybrid thinking (Claude 5, DeepSeek V4, GPT-6/o3/o4, Gemini 3.8) | Often stripped, broken, or dropped in SSE |
| **Dashboard UI** | Pure Leptos WebAssembly SPA with 35 languages | Heavy React/Webpack bundles |
| **Concurrency Model** | Work-stealing multi-threaded Tokio runtime | Single-threaded Event Loop bottlenecked on I/O |

---

## 🔄 Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│               Coding Agents & IDEs (Claude Code, Cursor, Cline, Roo, Codex...)   │
└────────────────────────────────────────┬────────────────────────────────────────┘
                                         │  HTTP / SSE (localhost:20128/v1)
                                         ▼
┌─────────────────────────────────────────────────────────────────────────────────┐
│                    Cloudflare Pingora Gateway (:20128)                          │
│   • Sub-50µs Fast-Path Dispatch       • Hardware Rate Limiting Token Bucket     │
│   • TLS Termination & Reverse Proxy   • Zero-copy Request / Response Pipeline   │
└───────────────┬─────────────────────────────────────────┬───────────────────────┘
                │                                         │
                ▼                                         ▼
┌───────────────────────────────┐       ┌─────────────────────────────────────────┐
│     Runtime Actix Engine      │       │      State Actix & Dynamic Config       │
│  • RTK Token Saver (20-40%)   │       │  • Arc-Shared Routing Contexts          │
│  • SSE Stream Direct Writer   │◄──────┤  • Multi-Account Credential Store       │
│  • Reasoning Chain Extractor  │       │  • Real-Time Quota & Reset Timers       │
│  • Anthropic ↔ OpenAI Bridge  │       │  • Zero-Copy Snapshot Projections       │
└───────────────┬───────────────┘       └─────────────────────────────────────────┘
                │
                ├─► [Tier 1: SUBSCRIPTION] Claude Opus 5 / Sonnet 4.6, GPT-6 Astra, GitHub Copilot
                │   ↓ Quota reached / 429 rate limit
                ├─► [Tier 2: CHEAP] DeepSeek V4, GLM-5.2, MiniMax M3, Groq, Mistral
                │   ↓ Budget limit reached
                └─► [Tier 3: FREE & LOCAL] Kiro AI, OpenCode Free, Vertex Free, Ollama, vLLM

Result: Sub-millisecond routing, zero dropped coding sessions, minimum cost, maximum speed.
```

---

## ⚡ Quick Start

### 1. Launch NullRouter

**Using Cargo (from source):**
```bash
git clone https://github.com/nullrouter/nullrouter.git
cd nullrouter
cargo run --release
```

**Using the All-in-One Startup Script:**
```bash
./run.sh
```

**Using the Single Binary (recommended):**

The supervisor forks all 7 microservices into child processes, using  as the runtime directory. All ports are derived from  (default: 20128).

**Using Docker:**
```bash
docker compose up -d
```

🎉 The dashboard and proxy will be live at:
- **API Base URL**: `http://localhost:20128/v1`
- **Dashboard**: `http://localhost:20128/dashboard`
- **Streaming Chat Playground**: `http://localhost:20128/dashboard/chat`

---

### 2. Connect Your Providers

Open `http://localhost:20128/dashboard` in your browser:
1. Navigate to **Providers**.
2. Add your API keys (e.g. Anthropic, OpenAI, DeepSeek, Google Gemini) or toggle **Free / Local Providers** (Kiro AI, OpenCode Free, Ollama).
3. Set your preferred routing order (Subscription ➔ Cheap ➔ Free).

---

### 3. Configure Your IDE / CLI Tool

#### 🔹 Claude Code
```bash
export ANTHROPIC_BASE_URL="http://localhost:20128"
export ANTHROPIC_API_KEY="nullrouter-local"
claude
```

#### 🔹 Cursor
1. Go to **Cursor Settings** ➔ **Models**.
2. Enable **OpenAI API Key** and set to `nullrouter-local`.
3. Override **OpenAI Base URL**: `http://localhost:20128/v1`.
4. Add desired model names (e.g., `claude-sonnet-4-6`, `deepseek-v4-pro`, `gpt-5.6` — see `models.dev`).

#### 🔹 Cline (VS Code Extension)
1. Open Cline Settings.
2. Select Provider: **OpenAI Compatible**.
3. **Base URL**: `http://localhost:20128/v1`
4. **API Key**: `nullrouter-local`
5. **Model ID**: `auto` (or `claude-sonnet-4-6`, `deepseek-v4-pro`).

#### 🔹 Roo Code
1. Open Roo Code Settings.
2. Select Provider: **Anthropic** or **OpenAI Compatible**.
3. For OpenAI Compatible: Base URL `http://localhost:20128/v1`.
4. Enable streaming and smart thinking fallback.

#### 🔹 OpenAI Codex CLI
```bash
export OPENAI_BASE_URL="http://localhost:20128/v1"
export OPENAI_API_KEY="nullrouter-local"
codex
```

#### 🔹 Continue.dev
In `~/.continue/config.json`:
```json
{
  "models": [
    {
      "title": "NullRouter Smart Route",
      "provider": "openai",
      "model": "auto",
      "apiBase": "http://localhost:20128/v1",
      "apiKey": "nullrouter-local"
    }
  ]
}
```

#### 🔹 Aider
```bash
aider --openai-api-base http://localhost:20128/v1 --openai-api-key nullrouter-local --model auto
```

---

## ✨ Key Features

### 1. 🗜️ RTK (Runtime Token-Kompact) Token Saver
Coding assistants burn up to 40% of their context windows on redundant tool outputs: giant `git diff` chunks, verbose directory scans, and repeated compiler errors.
- NullRouter inspects inbound `tool_result` and user messages in real time.
- Strips trailing noise, dedupes identical stack frames, and applies differential hunk compaction.
- Preserves 100% semantic fidelity while reducing token consumption by **20% to 40%**.

### 2. 🧠 Native Hybrid Extended Thinking & Reasoning
Supports state-of-the-art reasoning protocols without dropping chain-of-thought blocks:
- **Claude 4.6 / Opus 5 / Fable 5.1** (per `models.dev`): Automatic budget allocation (`medium` default = 8,192 tokens; `high` = 16,384 tokens). Dynamic `max_tokens` elevation above thinking budgets.
- **DeepSeek V4 Pro / V4 Flash**: Preserves `<think>` output tags and maps prompt caching (`prompt_cache_hit_tokens` to `cache_read_input_tokens`).
- **OpenAI GPT-6 Astra / o3 / o4**: Seamless parameter translation between `reasoning_effort` and standard chat parameters.

### 3. 🛡️ Resilient Multi-Tier Failover
Never hit a hard stop during critical refactors:
- **Tier 1 (Subscription)**: Exhaust your monthly subscription quota first.
- **Tier 2 (Cheap)**: On rate limit (429) or balance exhaustion, failover in < 1ms to DeepSeek, GLM, MiniMax, or Groq.
- **Tier 3 (Free / Local)**: Fallback seamlessly to Kiro AI, OpenCode Free, or local Ollama instances.

### 4. ⚡ Zero-Copy Cloudflare Pingora Architecture
- **No Node.js runtime bloat**: Pure native binary compiled with modern Rust 2024.
- **Sub-50µs Dispatch**: Fast-path bypass eliminates regex lookups and redundant header copies.
- **Direct SSE Writer**: Server-Sent Events write directly to output buffers without intermediate string allocations.

### 5. 🎨 Pure WebAssembly Dashboard with 35 Languages
- Built with **Leptos 0.7** running client-side WebAssembly for blazing-fast 60 FPS UI transitions.
- **Chat Playground** (`/dashboard/chat`): Stream responses with interactive collapsible cards for `<think>` reasoning traces.
- **35 Localized Languages**: English, Spanish, Mandarin, Japanese, German, French, Portuguese, Russian, Vietnamese, Korean, Arabic, and more.

---

## 🌐 Supported Providers

NullRouter integrates with **352 providers (1312 chat IDs, syncing via `models.dev` + OmniRoute catalog)** across all tiers:

| Provider | Supported Models | Reasoning Support | Prompt Caching | Free Tier Available |
| :--- | :--- | :---: | :---: | :---: |
| **Anthropic** | Claude Opus 5, Sonnet 4.6, Haiku 4.5 | ✅ (Hybrid) | ✅ | ❌ |
| **OpenAI** | GPT-6 Astra, GPT-5.6, o4-mini | ✅ | ✅ | ❌ |
| **DeepSeek** | DeepSeek V4 Pro, V4 Flash | ✅ (Native) | ✅ | ❌ (Ultra-cheap) |
| **Google Gemini** | Gemini 3.8 Flash, 3.7 Flash | ✅ | ✅ | ✅ (Free API tier) |
| **Groq** | Qwen3.8 27B, GPT-OSS 120B | ✅ | ❌ | ✅ (Generous free tier) |
| **Mistral AI** | Mistral Medium 3.5, Devstral 2, Codestral | ❌ | ❌ | ✅ |
| **MiniMax** | MiniMax M3, M2.5 | ❌ | ❌ | ❌ ($0.20/1M tokens) |
| **GLM (Zhipu)** | GLM-5.2, GLM-5.3 Flash | ❌ | ❌ | ✅ |
| **Kiro AI** | Claude Sonnet 4.6, MiniMax M3, GLM-5.2 | ❌ | ❌ | ✅ (Free monthly quota) |
| **OpenCode Free** | Qwen3.8 Coder, Llama 4 | ❌ | ❌ | ✅ (No signup required) |
| **Ollama / vLLM** | Any locally served GGUF/Safetensors | ✅ | ✅ | ✅ (100% Local & Free) |

---

## 📊 Benchmarks

*Benchmarked on an 8-core Linux system measuring proxying overhead and token throughput:*

| Metric | NullRouter (Rust / Pingora) | Node.js Router (Express / Axios) | Improvement |
| :--- | :--- | :--- | :--- |
| **Request Routing Overhead** | **42 µs** | 24,000 µs (24 ms) | **570x faster** |
| **Memory Footprint (Idle)** | **18 MB** | 142 MB | **87% lower** |
| **Memory Footprint (10k Conns)** | **64 MB** | 1.2 GB | **94% lower** |
| **Max Requests / Sec** | **45,200 req/s** | 2,100 req/s | **21x throughput** |
| **SSE Chunk Allocations** | **0 per event** | 3–5 heap allocs/chunk | **Zero allocations** |
| **Token Savings via RTK** | **24% – 38%** | 0% (Passthrough) | **Massive cost saving** |

---

## 📖 Complete Documentation & Wiki

Explore our in-depth guides in the [NullRouter GitHub Wiki](wiki/Home.md):

- 📘 [Getting Started & Installation Guide](wiki/Getting-Started.md)
- 🏗️ [Architecture & Performance Internals](wiki/Architecture-and-Performance.md)
- 💻 [Comprehensive Client Integrations (Claude Code, Cursor, Cline, Continue...)](wiki/Client-Integrations.md)
- 🔑 [Provider Configuration & Multi-Account Setup](wiki/Provider-Configuration.md)
- 🗜️ [RTK Token Saver Deep Dive](wiki/RTK-Token-Saver.md)
- 🔀 [Smart Routing & Combo Models](wiki/Smart-Routing-and-Failover.md)
- 🖥️ [Playground & WebAssembly Dashboard](wiki/Playground-and-WebAssembly-Dashboard.md)
- 📑 [Full REST & SSE API Reference](wiki/API-Reference.md)
- 📊 [Benchmarks](docs/BENCHMARKS.md)
- 🚀 [Single-Binary Mode](docs/Single-Binary.md)
- 🔒 [Security & Local Credential Encryption](wiki/Security-and-Privacy.md)
- 🩺 [Exhaustive Diagnostics & Troubleshooting Playbook](wiki/Diagnostics-and-Troubleshooting.md)
- 🌏 Localized Diagnostics: [中文诊断手册](wiki/i18n-Diagnostics-Guide-zh.md) • [Guía de Diagnóstico en Español](wiki/i18n-Diagnostics-Guide-es.md) • [日本語診断ガイド](wiki/i18n-Diagnostics-Guide-ja.md) • [Guia de Diagnóstico em Português](wiki/i18n-Diagnostics-Guide-pt.md) • [Hướng dẫn chẩn đoán Tiếng Việt](wiki/i18n-Diagnostics-Guide-vi.md)

---

## 🩺 Quick Diagnostics

Experiencing issues? Run through these quick checks:

```bash
# 1. Check if the gateway is running and listening on port 20128
curl -I http://localhost:20128/api/health

# 2. Check active provider connections
curl -s http://localhost:20128/api/state | jq '.connections | length'

# 3. Test unified model listing
curl -s http://localhost:20128/v1/models | jq '.data[].id'

# 4. Perform a test streaming completion
curl -X POST http://localhost:20128/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model":"auto","messages":[{"role":"user","content":"ping"}],"stream":true}'
```

For complete troubleshooting of port collisions (`E1001`), upstream timeouts (`E1003`), rate limits (`E2003`), or buffer framing issues, visit the [Diagnostics Playbook](wiki/Diagnostics-and-Troubleshooting.md).

---

## 📄 License

NullRouter is open source software licensed under the [Eclipse Public License 2.0](LICENSE).
