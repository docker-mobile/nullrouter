<div align="center">

# ⚡ NullRouter

**专为 AI 辅助编程打造的极速、零拷贝智能路由与 Token 优化器**

[![Rust](https://img.shields.io/badge/rust-edition%202024-orange.svg?logo=rust)](https://www.rust-lang.org)
[![Pingora](https://img.shields.io/badge/powered%20by-Cloudflare%20Pingora-blue.svg?logo=cloudflare)](https://github.com/cloudflare/pingora)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Latency](https://img.shields.io/badge/dispatch%20latency-%3C50%C2%B5s-brightgreen.svg)](https://github.com/nullrouter/nullrouter)
[![WebAssembly](https://img.shields.io/badge/UI-Leptos%20WASM-purple.svg?logo=webassembly)](apps/dashboard-leptos)
[![Providers](https://img.shields.io/badge/providers-40%2B%20integrated-success.svg)](../wiki/Provider-Configuration.md)

<p align="center">
  <b>永远告别编码中断。RTK 技术自动节省 20–40% 上下文 Token，毫秒级分发，智能故障转移覆盖 40+ 主流模型。</b>
</p>

[🚀 快速上手](#-快速上手) • [✨ 核心特性](#-核心特性) • [🔌 客户端集成](#-客户端集成) • [🌐 模型支持](#-支持的提供商) • [📊 性能对比](#-基准测试) • [📖 Wiki 文档](../wiki/Home.md) • [🩺 诊断排错](../wiki/Diagnostics-and-Troubleshooting.md)

---

### 🌐 其他语言版本

[English](../README.md) • [Español](README.es.md) • [简体中文](README.zh-CN.md) • [日本語](README.ja-JP.md) • [Português (Brasil)](README.pt-BR.md) • [Français](README.fr.md) • [Deutsch](README.de.md) • [Русский](README.ru.md) • [한국어](README.ko.md) • [Tiếng Việt](README.vi.md)

</div>

---

## 💡 为什么选择 NullRouter？

使用 Claude Code、Cursor、Cline、Roo Code、Codex 等 AI 编码工具时，开发者常面临高昂 Token 费用与频繁频率限制。基于 Node.js 或 Python 的传统路由工具存在显著的垃圾回收卡顿、高内存占用与数十毫秒延迟。

**NullRouter 基于 Rust 与 Cloudflare Pingora 底层内核全新构建：**

| 特性 | NullRouter (Rust + Pingora) | 传统 Node.js 路由器 |
| :--- | :--- | :--- |
| **代理引擎** | Cloudflare Pingora (零拷贝异步 HTTP) | Express / Koa / Fastify |
| **内部路由延迟** | **< 50 微秒** 极速直通 | 15 – 45 毫秒 |
| **静态内存占用** | **~18 MB** | 120 – 350 MB |
| **SSE 流式内存分配** | 直接缓冲区写入 (`0` 次中间分配) | 频繁字符串拼接与 JSON 解析 |
| **RTK Token 压缩** | 原生多阶段工具结果压缩 (省 20–40%) | 无压缩或简陋正则替换 |
| **推理协议规范化** | 完整支持 Claude 3.7、DeepSeek R1、o1/o3/o4 | 易丢失 `<think>` 思考链 |
| **控制台界面** | 纯 Leptos WebAssembly SPA (支持 35 种语言) | 庞大慢速的 Webpack 前端 |
| **并发模型** | Tokio 工作窃取多线程运行时 | 单线程 Event Loop 易受 I/O 阻塞 |

---

## 🔄 系统架构图

```
┌─────────────────────────────────────────────────────────────────────────────────┐
│              编码 Agent 与 IDE (Claude Code, Cursor, Cline, Roo, Codex...)       │
└────────────────────────────────────────┬────────────────────────────────────────┘
                                         │  HTTP / SSE (localhost:20128/v1)
                                         ▼
┌─────────────────────────────────────────────────────────────────────────────────┐
│                    Cloudflare Pingora 网关服务 (:20128)                         │
│   • Sub-50µs 极速转发                 • 硬件级令牌桶速率限制                     │
│   • TLS 终止与高可用反向代理          • 零拷贝请求/响应流水线                   │
└───────────────┬─────────────────────────────────────────┬───────────────────────┘
                │                                         │
                ▼                                         ▼
┌───────────────────────────────┐       ┌─────────────────────────────────────────┐
│      Runtime Actix 推理引擎   │       │       State Actix 状态与配置中心        │
│  • RTK Token 压缩器 (20-40%)  │       │  • Arc 共享路由上下文 (零分配克隆)       │
│  • SSE 流式直接缓冲区写入器   │◄──────┤  • 多账号凭据轮询管理                   │
│  • 深度思考链 (<think>) 提取  │       │  • 实时配额与自动重置定时器             │
│  • Anthropic ↔ OpenAI 双向桥接 │       │  • 零拷贝快照投影                       │
└───────────────┬───────────────┘       └─────────────────────────────────────────┘
                │
                ├─► [第 1 层级: 订阅套餐] Claude 3.7 / 3.5, OpenAI o3-mini, GitHub Copilot
                │   ↓ 达到配额上限或触发 429 速率限制
                ├─► [第 2 层级: 超低成本] DeepSeek R1/V3, GLM-4, MiniMax, Groq, Mistral
                │   ↓ 达到自定义预算上限
                └─► [第 3 层级: 免费与本地] Kiro AI, OpenCode Free, Vertex 免费层, Ollama, vLLM

最终成效：毫秒级路由响应、编码会话永不中断、极大节省 API 开支。
```

---

## ⚡ 快速上手

### 1. 启动 NullRouter

**源码编译运行 (需要 Rust 2024 环境):**
```bash
git clone https://github.com/nullrouter/nullrouter.git
cd nullrouter
cargo run --release
```

**使用一键启动脚本:**
```bash
./run.sh
```

**使用 Docker:**
```bash
docker compose up -d
```

🎉 启动后，服务将在以下端口生效：
- **API 接口地址**: `http://localhost:20128/v1`
- **管理控制台**: `http://localhost:20128/dashboard`
- **流式对话测试场**: `http://localhost:20128/dashboard/chat`

---

### 2. 配置提供商

在浏览器中打开 `http://localhost:20128/dashboard`：
1. 点击左侧 **Providers** (提供商管理)。
2. 输入你的 API Key (如 Anthropic, OpenAI, DeepSeek, Google Gemini) 或直接启用 **免费/本地模型** (Kiro AI, OpenCode Free, Ollama)。
3. 根据个人需求调整分层顺序 (订阅优先 ➔ 便宜模型 ➔ 免费兜底)。

---

### 3. 配置客户端与 IDE

#### 🔹 Claude Code
```bash
export ANTHROPIC_BASE_URL="http://localhost:20128"
export ANTHROPIC_API_KEY="nullrouter-local"
claude
```

#### 🔹 Cursor
1. 进入 **Cursor Settings** ➔ **Models**。
2. 开启 **OpenAI API Key**，填入 `nullrouter-local`。
3. 勾选并覆盖 **OpenAI Base URL**: `http://localhost:20128/v1`。
4. 添加需要使用的模型 (例如 `claude-3-7-sonnet`, `deepseek-r1`, `auto`)。

#### 🔹 Cline (VS Code 扩展)
1. 打开 Cline 设置。
2. Provider 选择：**OpenAI Compatible**。
3. **Base URL**: `http://localhost:20128/v1`
4. **API Key**: `nullrouter-local`
5. **Model ID**: `auto` (或指定具体模型)。

#### 🔹 Roo Code
1. 打开 Roo Code 设置。
2. Provider 选择 **OpenAI Compatible**。
3. Base URL 填入 `http://localhost:20128/v1`。
4. 勾选流式输出支持。

#### 🔹 Continue.dev
在 `~/.continue/config.json` 中配置：
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

---

## ✨ 核心亮点

1. **RTK (Runtime Token-Kompact) 工具输出智能压缩**：实时压缩 git diff、文件检索、编译器报错日志，在完全保留语义的前提下立省 20%~40% Token。
2. **混合深度思考 (Hybrid Extended Thinking) 协议规范化**：无缝对接 Claude 3.7、DeepSeek R1、o1/o3/o4，控制台 Chat Playground 实时展开/收起 `<think>` 思考过程。
3. **无缝三级故障转移**：遇到 429 限流或 5xx 故障时，在 <1ms 内无感切换下一个层级，保证编码会话不掉线。
4. **零拷贝 Cloudflare Pingora 内核**：彻底摒弃 Node.js 虚拟机与 V8 垃圾回收，单机支持超 40,000+ QPS 吞吐。
5. **多语言 Leptos WASM 控制台**：完全在浏览器端以 WebAssembly 原生渲染，内建中文、英语、日语、西班牙语等 35 种本地化语言支持。

---

## 📖 完整文档与指南

请查阅 [NullRouter GitHub Wiki](../wiki/Home.md) 获取更多深度指南：
- 📘 [快速安装与部署手册](../wiki/Getting-Started.md)
- 🏗️ [Pingora 内核与零拷贝架构详解](../wiki/Architecture-and-Performance.md)
- 💻 [全客户端对接指南 (Cursor / Cline / Roo / Codex / Aider)](../wiki/Client-Integrations.md)
- 🔑 [40+ 提供商与多账号轮询配置](../wiki/Provider-Configuration.md)
- 🗜️ [RTK Token 压缩算法与原理](../wiki/RTK-Token-Saver.md)
- 📑 [OpenAI / Anthropic REST & SSE API 参考](../wiki/API-Reference.md)
- 🩺 [详尽故障排查与诊断手册](../wiki/Diagnostics-and-Troubleshooting.md)
- 🇨🇳 [中文专版系统诊断指南](../wiki/i18n-Diagnostics-Guide-zh.md)

---

## 📄 开源许可证

NullRouter 采用 [MIT 开源许可证](LICENSE)。
