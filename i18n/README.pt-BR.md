<div align="center">

# ⚡ NullRouter

**Roteador de IA e Otimizador de Tokens de Ultra Alto Desempenho e Cópia Zero para Codificação Agêntica**

[![Rust](https://img.shields.io/badge/rust-edi%C3%A7%C3%A3o%202024-orange.svg?logo=rust)](https://www.rust-lang.org)
[![Pingora](https://img.shields.io/badge/desenvolvido%20com-Cloudflare%20Pingora-blue.svg?logo=cloudflare)](https://github.com/cloudflare/pingora)
[![License: EPL-2.0](https://img.shields.io/badge/License-EPL--2.0-blue.svg)](LICENSE)
[![Latency](https://img.shields.io/badge/lat%C3%AAncia%20de%20despacho-%3C50%C2%B5s-brightgreen.svg)](https://github.com/nullrouter/nullrouter)
[![WebAssembly](https://img.shields.io/badge/UI-Leptos%20WASM-purple.svg?logo=webassembly)](apps/dashboard-leptos)
[![Providers](https://img.shields.io/badge/provedores-40%2B%20integrados-success.svg)](../wiki/Provider-Configuration.md)

<p align="center">
  <b>Nunca interrompa sua programação. Economize de 20% a 40% de tokens com RTK, latência submilisegundo e failover inteligente entre mais de 40 provedores.</b>
</p>

[🚀 Início Rápido](#-in%C3%ADcio-r%C3%A1pido) • [✨ Recursos](#-principais-recursos) • [🔌 Integração](#-integra%C3%A7%C3%A3o-com-clientes) • [🌐 Provedores](#-provedores-suportados) • [📊 Benchmarks](#-benchmarks) • [📖 Wiki](../wiki/Home.md) • [🩺 Diagnóstico](../wiki/Diagnostics-and-Troubleshooting.md)

---

### 🌐 Ler em outros idiomas

[English](../README.md) • [Español](README.es.md) • [简体中文](README.zh-CN.md) • [日本語](README.ja-JP.md) • [Português (Brasil)](README.pt-BR.md) • [Français](README.fr.md) • [Deutsch](README.de.md) • [Русский](README.ru.md) • [한국어](README.ko.md) • [Tiếng Việt](README.vi.md)

</div>

---

## 💡 Por que o NullRouter?

Fluxos modernos com agentes de codificação (Claude Code, Cursor, Cline, Roo Code, Codex) executam loops massivos de requisições. Roteadores tradicionais em Node.js ou Python geram pausas de garbage collection, alto consumo de memória e dezenas de milissegundos de latência.

**O NullRouter foi construído do zero em Rust com o motor Pingora da Cloudflare:**

| Recurso | NullRouter (Rust + Pingora) | Roteadores Node.js Tradicionais |
| :--- | :--- | :--- |
| **Motor Proxy** | Cloudflare Pingora (HTTP assíncrono zero-copy) | Express / Koa / Fastify |
| **Latência de Roteamento** | **< 50 microsegundos** caminho rápido | 15 – 45 milissegundos |
| **Uso de Memória** | **~18 MB** em repouso | 120 – 350 MB |
| **Alocação de Streaming SSE** | Escrita direta no buffer (`0` alocações intermediárias) | Concatenações contínuas de string e JSON |
| **Compressão RTK** | Compactação diferencial de diffs (economia de 20–40%) | Sem compressão ou regex simples |
| **Normalização de Raciocínio**| Suporte nativo Claude 4, DeepSeek R1/V3.2, o3/o4 | Perda frequente de cadeias `<think>` |
| **Dashboard** | Leptos WebAssembly SPA com 35 idiomas | Bundles pesados em React/Webpack |

---

## ⚡ Início Rápido

### 1. Iniciar o NullRouter

```bash
git clone https://github.com/nullrouter/nullrouter.git
cd nullrouter
cargo run --release
```

Ou usando o script de inicialização:
```bash
./run.sh
```

Acessos:
- **Endpoint API**: `http://localhost:20128/v1`
- **Dashboard**: `http://localhost:20128/dashboard`
- **Chat Playground**: `http://localhost:20128/dashboard/chat`

---

### 2. Configurar seus Clientes

#### 🔹 Claude Code
```bash
export ANTHROPIC_BASE_URL="http://localhost:20128"
export ANTHROPIC_API_KEY="nullrouter-local"
claude
```

#### 🔹 Cursor
1. Acesse **Cursor Settings** ➔ **Models**.
2. Ative **OpenAI API Key** e defina como `nullrouter-local`.
3. Sobrescreva **OpenAI Base URL**: `http://localhost:20128/v1`.
4. Adicione os modelos desejados (`claude-sonnet-4-6`, `deepseek-v4-pro`, `auto`).

---

## 📖 Documentação Completa na Wiki

Consulte a [NullRouter GitHub Wiki](../wiki/Home.md) para guias técnicos detalhados:
- 📘 [Guia de Instalação e Inicialização](../wiki/Getting-Started.md)
- 🏗️ [Arquitetura Pingora e Zero-Copy Internals](../wiki/Architecture-and-Performance.md)
- 💻 [Guia de Integração para IDEs](../wiki/Client-Integrations.md)
- 🔑 [Configuração de Provedores e Múltiplas Contas](../wiki/Provider-Configuration.md)
- 🩺 [Playbook Completo de Diagnóstico](../wiki/Diagnostics-and-Troubleshooting.md)
- 🇧🇷 [Guia de Diagnóstico em Português](../wiki/i18n-Diagnostics-Guide-pt.md)

---

## 📄 Licença

NullRouter é distribuído sob licença [EPL-2.0](LICENSE).
