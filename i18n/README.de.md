<div align="center">

# ⚡ NullRouter

**Ultra-Hochleistungsfähiger, Zero-Copy KI-Router & Token-Optimierer für agentisches Coden**

[![Rust](https://img.shields.io/badge/rust-edition%202024-orange.svg?logo=rust)](https://www.rust-lang.org)
[![Pingora](https://img.shields.io/badge/powered%20by-Cloudflare%20Pingora-blue.svg?logo=cloudflare)](https://github.com/cloudflare/pingora)
[![License: EPL-2.0](https://img.shields.io/badge/License-EPL--2.0-blue.svg)](LICENSE)
[![Latency](https://img.shields.io/badge/routing%20latenz-%3C50%C2%B5s-brightgreen.svg)](https://github.com/nullrouter/nullrouter)
[![WebAssembly](https://img.shields.io/badge/UI-Leptos%20WASM-purple.svg?logo=webassembly)](apps/dashboard-leptos)
[![Providers](https://img.shields.io/badge/provider-40%2B%20integriert-success.svg)](../wiki/Provider-Configuration.md)

<p align="center">
  <b>Nie wieder Programmierunterbrechungen. Sparen Sie 20–40 % Tokens mit RTK, Sub-Millisekunden-Dispatch und intelligentem Failover über mehr als 40 KI-Provider.</b>
</p>

[🚀 Schnellstart](#-schnellstart) • [✨ Hauptfunktionen](#-hauptfunktionen) • [🔌 Client-Integration](#-client-integration) • [🌐 Unterstützte Provider](#-unterst%C3%BCtzte-provider) • [📊 Benchmarks](#-benchmarks) • [📖 Wiki-Dokumentation](../wiki/Home.md) • [🩺 Fehlerdiagnose](../wiki/Diagnostics-and-Troubleshooting.md)

---

### 🌐 In anderen Sprachen lesen

[English](../README.md) • [Español](README.es.md) • [简体中文](README.zh-CN.md) • [日本語](README.ja-JP.md) • [Português (Brasil)](README.pt-BR.md) • [Français](README.fr.md) • [Deutsch](README.de.md) • [Русский](README.ru.md) • [한국어](README.ko.md) • [Tiếng Việt](README.vi.md)

</div>

---

## 💡 Warum NullRouter?

Moderne KI-Coding-Agenten (Claude Code, Cursor, Cline, Roo Code, Codex) führen endlose Prompt-Schleifen aus. Bisherige Node.js- oder Python-basierte Proxies verursachen Garbage-Collection-Ruckler, hohen Speicherverbrauch und zweistellige Millisekunden-Latenzen.

**NullRouter basiert nativ auf Rust 2024 und Cloudflares Pingora-Proxy-Engine:**

| Merkmal | NullRouter (Rust + Pingora) | Herkömmliche Node.js Router |
| :--- | :--- | :--- |
| **Proxy-Engine** | Cloudflare Pingora (Zero-Copy Async HTTP) | Express / Koa / Fastify |
| **Interne Routing-Latenz** | **< 50 Mikrosekunden** Fast-Path | 15 – 45 Millisekunden |
| **Speicherverbrauch** | **~18 MB** im Leerlauf | 120 – 350 MB |
| **SSE-Stream-Allokation**| Direkter Buffer-Writer (`0` Zwischenallokationen) | Ständige String-Verkettungen und JSON-Parsen |
| **RTK-Token-Komprimierung** | Native Diff-Kompaktierung (20–40% Ersparnis) | Keine Kompression |
| **Reasoning-Unterstützung** | Vollständig für Claude 4, DeepSeek R1/V3.2, o3/o4 | Häufiger Verlust von `<think>`-Ketten |
| **Dashboard** | Leptos WebAssembly SPA in 35 Sprachen | Aufgeblähte Webpack/React-Bundles |

---

## ⚡ Schnellstart

### 1. NullRouter starten

```bash
git clone https://github.com/nullrouter/nullrouter.git
cd nullrouter
cargo run --release
```

Oder mit dem Startskript:
```bash
./run.sh
```

Endpunkte:
- **API-Endpunkt**: `http://localhost:20128/v1`
- **Dashboard**: `http://localhost:20128/dashboard`
- **Chat-Playground**: `http://localhost:20128/dashboard/chat`

---

### 2. Client konfigurieren

#### 🔹 Claude Code
```bash
export ANTHROPIC_BASE_URL="http://localhost:20128"
export ANTHROPIC_API_KEY="nullrouter-local"
claude
```

#### 🔹 Cursor
1. Öffnen Sie **Cursor Settings** ➔ **Models**.
2. Setzen Sie **OpenAI API Key** auf `nullrouter-local`.
3. Überschreiben Sie **OpenAI Base URL**: `http://localhost:20128/v1`.
4. Tragen Sie die gewünschten Modelle ein (z. B. `claude-sonnet-4-6`, `deepseek-v4-pro`, `auto`).

---

## 📖 Ausführliche Wiki-Dokumentation

Weitere detaillierte Anleitungen finden Sie im [NullRouter GitHub Wiki](../wiki/Home.md):
- 📘 [Installationsanleitung & Setup](../wiki/Getting-Started.md)
- 🏗️ [Pingora-Architektur & Zero-Copy Internals](../wiki/Architecture-and-Performance.md)
- 💻 [IDE- & Client-Konfigurationen](../wiki/Client-Integrations.md)
- 🔑 [Provider-Einrichtung & Multi-Account-Pooling](../wiki/Provider-Configuration.md)
- 🩺 [Umfassendes Diagnose- und Fehlerbehebungshandbuch](../wiki/Diagnostics-and-Troubleshooting.md)

---

## 📄 Lizenz

NullRouter ist freie Open-Source-Software unter der [EPL-2.0](LICENSE).
