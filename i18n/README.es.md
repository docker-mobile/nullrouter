<div align="center">

# ⚡ NullRouter

**Enrutador de IA y Optimizador de Tokens de Ultra Alto Rendimiento y Copia Cero para Programación Agéntica**

[![Rust](https://img.shields.io/badge/rust-edici%C3%B3n%202024-orange.svg?logo=rust)](https://www.rust-lang.org)
[![Pingora](https://img.shields.io/badge/impulsado%20por-Cloudflare%20Pingora-blue.svg?logo=cloudflare)](https://github.com/cloudflare/pingora)
[![License: MIT](https://img.shields.io/badge/Licencia-MIT-yellow.svg)](LICENSE)
[![Latency](https://img.shields.io/badge/latencia%20de%20despacho-%3C50%C2%B5s-brightgreen.svg)](https://github.com/nullrouter/nullrouter)
[![WebAssembly](https://img.shields.io/badge/UI-Leptos%20WASM-purple.svg?logo=webassembly)](apps/dashboard-leptos)
[![Providers](https://img.shields.io/badge/proveedores-40%2B%20integrados-success.svg)](../wiki/Provider-Configuration.md)

<p align="center">
  <b>Nunca detengas tu flujo de código. Ahorra 20–40% de tokens con RTK, despacho submilisegundo y conmutación por error inteligente en más de 40 proveedores.</b>
</p>

[🚀 Inicio Rápido](#-inicio-rápido) • [✨ Características](#-características-principales) • [🔌 Integración](#-integración-con-clientes) • [🌐 Proveedores](#-proveedores-compatibles) • [📊 Rendimiento](#-pruebas-de-rendimiento) • [📖 Wiki](../wiki/Home.md) • [🩺 Diagnóstico](../wiki/Diagnostics-and-Troubleshooting.md)

---

### 🌐 Leer en otros idiomas

[English](../README.md) • [Español](README.es.md) • [简体中文](README.zh-CN.md) • [日本語](README.ja-JP.md) • [Português (Brasil)](README.pt-BR.md) • [Français](README.fr.md) • [Deutsch](README.de.md) • [Русский](README.ru.md) • [한국어](README.ko.md) • [Tiếng Việt](README.vi.md)

</div>

---

## 💡 ¿Por qué NullRouter?

Los flujos de trabajo modernos con agentes de código (Claude Code, Cursor, Cline, Roo Code, Codex) realizan bucles de peticiones masivas. Los enrutadores convencionales basados en Node.js o Python introducen pausas de recolección de basura, alto consumo de memoria y decenas de milisegundos de latencia.

**NullRouter está construido desde cero en Rust sobre el motor Cloudflare Pingora:**

| Característica | NullRouter (Rust + Pingora) | Enrutadores Node.js Tradicionales |
| :--- | :--- | :--- |
| **Motor Proxy** | Cloudflare Pingora (HTTP asíncrono zero-copy) | Express / Koa / Fastify |
| **Latencia de Enrutamiento** | **< 50 microsegundos** vía rápida | 15 – 45 milisegundos |
| **Uso de Memoria** | **~18 MB** en reposo | 120 – 350 MB |
| **Asignación en SSE** | Escritor directo a búfer (`0` copias intermedias) | Concatenación de strings y bucles JSON |
| **Compresor RTK** | Compresión multipaso de diffs (ahorro 20–40%) | Sin compresión o expresiones regulares simples |
| **Normalización de Razonamiento** | Soporte Claude 3.7, DeepSeek R1, o1/o3/o4 | Pérdida frecuente de cadenas `<think>` |
| **Interfaz de Usuario** | Leptos WebAssembly SPA en 35 idiomas | Paquetes pesados de Webpack/React |

---

## ⚡ Inicio Rápido

### 1. Iniciar NullRouter

**Compilación desde código fuente (Rust 2024):**
```bash
git clone https://github.com/nullrouter/nullrouter.git
cd nullrouter
cargo run --release
```

**Script de inicio directo:**
```bash
./run.sh
```

**Docker:**
```bash
docker compose up -d
```

Puntos de acceso activos:
- **API Base**: `http://localhost:20128/v1`
- **Panel de Control**: `http://localhost:20128/dashboard`
- **Playground de Chat en Tiempo Real**: `http://localhost:20128/dashboard/chat`

---

### 2. Configurar en tus Herramientas de Desarrollo

#### 🔹 Claude Code
```bash
export ANTHROPIC_BASE_URL="http://localhost:20128"
export ANTHROPIC_API_KEY="nullrouter-local"
claude
```

#### 🔹 Cursor
1. Abre **Cursor Settings** ➔ **Models**.
2. Activa **OpenAI API Key**: `nullrouter-local`.
3. Configura **OpenAI Base URL**: `http://localhost:20128/v1`.
4. Ingresa el modelo preferido (por ejemplo: `claude-3-7-sonnet`, `deepseek-r1` o `auto`).

#### 🔹 Cline (Extensión de VS Code)
1. Abre los ajustes de Cline.
2. Proveedor: **OpenAI Compatible**.
3. **Base URL**: `http://localhost:20128/v1`
4. **API Key**: `nullrouter-local`
5. **Model ID**: `auto`.

---

## 📖 Documentación Completa en la Wiki

- 📘 [Guía de Inicio e Instalación](../wiki/Getting-Started.md)
- 🏗️ [Arquitectura Pingora y Cero Copia](../wiki/Architecture-and-Performance.md)
- 💻 [Guía de Integración con Clientes](../wiki/Client-Integrations.md)
- 🔑 [Configuración de más de 40 Proveedores](../wiki/Provider-Configuration.md)
- 🩺 [Manual Exhaustivo de Diagnóstico y Errores](../wiki/Diagnostics-and-Troubleshooting.md)
- 🇪🇸 [Guía de Diagnóstico en Español](../wiki/i18n-Diagnostics-Guide-es.md)

---

## 📄 Licencia

NullRouter es software de código abierto bajo licencia [MIT](LICENSE).
