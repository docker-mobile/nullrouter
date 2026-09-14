<div align="center">

# ⚡ NullRouter

**Сверхбыстрый zero-copy ИИ-маршрутизатор и оптимизатор токенов для агентной разработки**

[![Rust](https://img.shields.io/badge/rust-%D1%80%D0%B5%D0%B4%D0%B0%D0%BA%D1%86%D0%B8%D1%8F%202024-orange.svg?logo=rust)](https://www.rust-lang.org)
[![Pingora](https://img.shields.io/badge/%D0%BD%D0%B0%20%D0%B1%D0%B0%D0%B7%D0%B5-Cloudflare%20Pingora-blue.svg?logo=cloudflare)](https://github.com/cloudflare/pingora)
[![License: MIT](https://img.shields.io/badge/%D0%9B%D0%B8%D1%86%D0%B5%D0%BD%D0%B7%D0%B8%D1%8F-MIT-yellow.svg)](LICENSE)
[![Latency](https://img.shields.io/badge/%D0%B7%D0%B0%D0%B4%D0%B5%D1%80%D0%B6%D0%BA%D0%B0%20%D0%BC%D0%B0%D1%80%D1%88%D1%80%D1%83%D1%82%D0%B8%D0%B7%D0%B0%D1%86%D0%B8%D0%B8-%3C50%C2%B5s-brightgreen.svg)](https://github.com/nullrouter/nullrouter)
[![WebAssembly](https://img.shields.io/badge/UI-Leptos%20WASM-purple.svg?logo=webassembly)](apps/dashboard-leptos)
[![Providers](https://img.shields.io/badge/%D0%BF%D1%80%D0%BE%D0%B2%D0%B0%D0%B9%D0%B4%D0%B5%D1%80%D1%8B-40%2B%20%D0%B8%D0%BD%D1%82%D0%B5%D0%B3%D1%80%D0%B8%D1%80%D0%BE%D0%B2%D0%B0%D0%BD%D0%BE-success.svg)](../wiki/Provider-Configuration.md)

<p align="center">
  <b>Никаких прерываний при кодинге. Экономьте 20–40% токенов с RTK, задержка менее 1 мс и умный отказоустойчивый переход между 40+ провайдерами ИИ.</b>
</p>

[🚀 Быстрый старт](#-быстрый-старт) • [✨ Особенности](#-ключевые-особенности) • [🔌 Клиенты](#-интеграция-с-клиентами) • [🌐 Провайдеры](#-провайдеры) • [📊 Тесты](#-бенчмарки) • [📖 Документация Wiki](../wiki/Home.md) • [🩺 Диагностика](../wiki/Diagnostics-and-Troubleshooting.md)

---

### 🌐 Читать на других языках

[English](../README.md) • [Español](README.es.md) • [简体中文](README.zh-CN.md) • [日本語](README.ja-JP.md) • [Português (Brasil)](README.pt-BR.md) • [Français](README.fr.md) • [Deutsch](README.de.md) • [Русский](README.ru.md) • [한국어](README.ko.md) • [Tiếng Việt](README.vi.md)

</div>

---

## 💡 Почему NullRouter?

Агентные системы (Claude Code, Cursor, Cline, Roo Code, Codex) выполняют бесконечные циклы запросов. Традиционные прокси на Node.js или Python создают паузы сборщика мусора, высокое потребление памяти и задержки в десятки миллисекунд.

**NullRouter полностью написан на Rust поверх движка Cloudflare Pingora:**

| Возможность | NullRouter (Rust + Pingora) | Традиционные роутеры на Node.js |
| :--- | :--- | :--- |
| **Прокси-движок** | Cloudflare Pingora (асинхронный zero-copy HTTP) | Express / Koa / Fastify |
| **Внутренняя задержка** | **< 50 микросекунд** (fast-path) | 15 – 45 миллисекунд |
| **Память (idle)** | **~18 МБ** | 120 – 350 МБ |
| **Аллокация в SSE** | Прямая запись в буфер (`0` промежуточных копий) | Конкатенация строк и частый парсинг JSON |
| **Сжатие токенов RTK** | Нативная дифференциальная компактизация (20–40%) | Отсутствует |
| **Нормализация рассуждений**| Полная поддержка Claude 3.7, DeepSeek R1, o1/o3/o4 | Часто теряются теги `<think>` |
| **Панель управления** | Легкий Leptos WebAssembly SPA на 35 языках | Тяжелые бандлы Webpack/React |

---

## ⚡ Быстрый старт

### 1. Запуск NullRouter

```bash
git clone https://github.com/nullrouter/nullrouter.git
cd nullrouter
cargo run --release
```

Либо через скрипт:
```bash
./run.sh
```

Интерфейсы:
- **API URL**: `http://localhost:20128/v1`
- **Панель управления**: `http://localhost:20128/dashboard`
- **Чат-песочница**: `http://localhost:20128/dashboard/chat`

---

### 2. Настройка в редакторах

#### 🔹 Claude Code
```bash
export ANTHROPIC_BASE_URL="http://localhost:20128"
export ANTHROPIC_API_KEY="nullrouter-local"
claude
```

#### 🔹 Cursor
1. Перейдите в **Cursor Settings** ➔ **Models**.
2. Установите **OpenAI API Key**: `nullrouter-local`.
3. Переопределите **OpenAI Base URL**: `http://localhost:20128/v1`.
4. Укажите модели (например, `claude-3-7-sonnet`, `deepseek-r1` или `auto`).

---

## 📖 База знаний и Wiki

Полная документация доступна в [NullRouter GitHub Wiki](../wiki/Home.md):
- 📘 [Руководство по установке и настройке](../wiki/Getting-Started.md)
- 🏗️ [Архитектура Pingora и Zero-Copy под капотом](../wiki/Architecture-and-Performance.md)
- 💻 [Интеграция со всеми средами разработки](../wiki/Client-Integrations.md)
- 🔑 [Настройка 40+ провайдеров и пулов аккаунтов](../wiki/Provider-Configuration.md)
- 🩺 [Подробное руководство по диагностике и ошибкам](../wiki/Diagnostics-and-Troubleshooting.md)

---

## 📄 Лицензия

NullRouter лицензирован под открытой лицензией [MIT](LICENSE).
