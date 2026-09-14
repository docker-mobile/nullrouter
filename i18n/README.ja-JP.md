<div align="center">

# ⚡ NullRouter

**エージェント型コーディングのための超高速・ゼロコピーAIルーター＆トークン最適化エンジン**

[![Rust](https://img.shields.io/badge/rust-edition%202024-orange.svg?logo=rust)](https://www.rust-lang.org)
[![Pingora](https://img.shields.io/badge/powered%20by-Cloudflare%20Pingora-blue.svg?logo=cloudflare)](https://github.com/cloudflare/pingora)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Latency](https://img.shields.io/badge/dispatch%20latency-%3C50%C2%B5s-brightgreen.svg)](https://github.com/nullrouter/nullrouter)
[![WebAssembly](https://img.shields.io/badge/UI-Leptos%20WASM-purple.svg?logo=webassembly)](apps/dashboard-leptos)
[![Providers](https://img.shields.io/badge/providers-40%2B%20integrated-success.svg)](../wiki/Provider-Configuration.md)

<p align="center">
  <b>コーディングを止めない。RTK技術でコンテキストトークンを20〜40%削減。サブミリ秒の高速ディスパッチと40以上のAIプロバイダー自動フェイルオーバー。</b>
</p>

[🚀 クイックスタート](#-クイックスタート) • [✨ 主な特徴](#-主な特徴) • [🔌 クライアント設定](#-クライアント連携) • [🌐 対応プロバイダー](#-対応プロバイダー) • [📊 ベンチマーク](#-ベンチマーク) • [📖 Wiki ドキュメント](../wiki/Home.md) • [🩺 診断ガイド](../wiki/Diagnostics-and-Troubleshooting.md)

---

### 🌐 他の言語で読む

[English](../README.md) • [Español](README.es.md) • [简体中文](README.zh-CN.md) • [日本語](README.ja-JP.md) • [Português (Brasil)](README.pt-BR.md) • [Français](README.fr.md) • [Deutsch](README.de.md) • [Русский](README.ru.md) • [한국어](README.ko.md) • [Tiếng Việt](README.vi.md)

</div>

---

## 💡 なぜ NullRouter なのか？

Claude Code、Cursor、Cline、Roo Code、Codexなどのエージェントツールは、膨大なプロンプトとツール出力を消費します。Node.jsやPythonで作られた既存のプロキシは、ガベージコレクションによるスパイク、過大なメモリ消費、数十ミリ秒のレイテンシを引き起こします。

**NullRouterは、Cloudflare PingoraエンジンをベースにRust 2024でゼロから設計されています：**

| 機能 | NullRouter (Rust + Pingora) | 従来のNode.jsルーター |
| :--- | :--- | :--- |
| **プロキシエンジン** | Cloudflare Pingora (ゼロコピー非同期HTTP) | Express / Koa / Fastify |
| **ルーティング遅延** | **< 50マイクロ秒** （ファストパス） | 15 〜 45ミリ秒 |
| **常駐メモリ使用量** | **~18 MB** | 120 〜 350 MB |
| **SSEストリーミング割り当て** | 出力バッファ直接書き込み（中間アロケーション `0`） | 頻繁な文字列結合とJSONパース |
| **RTK トークン圧縮** | ネイティブ差分圧縮（20〜40%節約） | 圧縮なし、または単純な正規表現 |
| **思考プロセス正規化** | Claude 3.7、DeepSeek R1、o1/o3/o4ネイティブ対応 | `<think>` タグが喪失しやすい |
| **ダッシュボード UI** | Leptos WebAssembly SPA（35言語対応） | 巨大なReact/Webpackバンドル |

---

## ⚡ クイックスタート

### 1. NullRouter の起動

**ソースコードからビルド (Rust 2024):**
```bash
git clone https://github.com/nullrouter/nullrouter.git
cd nullrouter
cargo run --release
```

**起動スクリプトの利用:**
```bash
./run.sh
```

**Docker の利用:**
```bash
docker compose up -d
```

エンドポイント:
- **API エンドポイント**: `http://localhost:20128/v1`
- **ダッシュボード**: `http://localhost:20128/dashboard`
- **チャットプレイグラウンド**: `http://localhost:20128/dashboard/chat`

---

### 2. クライアントツールの設定

#### 🔹 Claude Code
```bash
export ANTHROPIC_BASE_URL="http://localhost:20128"
export ANTHROPIC_API_KEY="nullrouter-local"
claude
```

#### 🔹 Cursor
1. **Cursor Settings** ➔ **Models** を開く。
2. **OpenAI API Key** を有効にし、`nullrouter-local` と入力。
3. **OpenAI Base URL** を上書き: `http://localhost:20128/v1`。
4. 使用したいモデル名（例: `claude-3-7-sonnet`, `deepseek-r1`, `auto`）を追加。

#### 🔹 Cline (VS Code 拡張機能)
1. Cline 設定を開く。
2. プロバイダー: **OpenAI Compatible** を選択。
3. **Base URL**: `http://localhost:20128/v1`
4. **API Key**: `nullrouter-local`
5. **Model ID**: `auto`

---

## 📖 GitHub Wiki 完全ガイド

詳細な設定や運用方法については [NullRouter GitHub Wiki](../wiki/Home.md) をご覧ください：
- 📘 [導入・環境構築ガイド](../wiki/Getting-Started.md)
- 🏗️ [Pingoraコアとゼロコピーアーキテクチャ内部解説](../wiki/Architecture-and-Performance.md)
- 💻 [各IDE・CLI連携設定集](../wiki/Client-Integrations.md)
- 🔑 [40以上のプロバイダー設定と複数アカウント分散](../wiki/Provider-Configuration.md)
- 🩺 [網羅的トラブルシューティング・エラー辞書](../wiki/Diagnostics-and-Troubleshooting.md)
- 🇯🇵 [日本語版システム診断ガイド](../wiki/i18n-Diagnostics-Guide-ja.md)

---

## 📄 ライセンス

NullRouter は [MIT License](LICENSE) の下で公開されているオープンソースソフトウェアです。
