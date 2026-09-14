<div align="center">

# ⚡ NullRouter

**에이전트 코딩을 위한 초고성능, 제로카피 AI 라우터 & 토큰 최적화 엔진**

[![Rust](https://img.shields.io/badge/rust-edition%202024-orange.svg?logo=rust)](https://www.rust-lang.org)
[![Pingora](https://img.shields.io/badge/powered%20by-Cloudflare%20Pingora-blue.svg?logo=cloudflare)](https://github.com/cloudflare/pingora)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)
[![Latency](https://img.shields.io/badge/dispatch%20latency-%3C50%C2%B5s-brightgreen.svg)](https://github.com/nullrouter/nullrouter)
[![WebAssembly](https://img.shields.io/badge/UI-Leptos%20WASM-purple.svg?logo=webassembly)](apps/dashboard-leptos)
[![Providers](https://img.shields.io/badge/providers-40%2B%20integrated-success.svg)](../wiki/Provider-Configuration.md)

<p align="center">
  <b>코딩 흐름을 멈추지 마세요. RTK 기술로 토큰을 20~40% 절약하고, 서브밀리초 단위의 빠른 디스패치와 40개 이상의 AI 제공자 자동 장애 조치를 지원합니다.</b>
</p>

[🚀 빠른 시작](#-빠른-시작) • [✨ 주요 기능](#-주요-기능) • [🔌 클라이언트 연동](#-클라이언트-연동) • [🌐 지원 제공자](#-지원-제공자) • [📊 벤치마크](#-벤치마크) • [📖 Wiki 문서](../wiki/Home.md) • [🩺 진단 가이드](../wiki/Diagnostics-and-Troubleshooting.md)

---

### 🌐 다른 언어로 읽기

[English](../README.md) • [Español](README.es.md) • [简体中文](README.zh-CN.md) • [日本語](README.ja-JP.md) • [Português (Brasil)](README.pt-BR.md) • [Français](README.fr.md) • [Deutsch](README.de.md) • [Русский](README.ru.md) • [한국어](README.ko.md) • [Tiếng Việt](README.vi.md)

</div>

---

## 💡 왜 NullRouter인가요?

Claude Code, Cursor, Cline, Roo Code, Codex와 같은 현대적인 코딩 에이전트는 수많은 프롬프트 루프를 실행합니다. Node.js 또는 Python 기반의 기존 프록시는 가비지 컬렉션 지연, 높은 메모리 점유율, 수십 밀리초의 오버헤드를 발생시킵니다.

**NullRouter는 Cloudflare Pingora 엔진을 기반으로 Rust 2024에서 직접 설계되었습니다:**

| 기능 | NullRouter (Rust + Pingora) | 기존 Node.js 라우터 |
| :--- | :--- | :--- |
| **프록시 엔진** | Cloudflare Pingora (비동기 제로카피 HTTP) | Express / Koa / Fastify |
| **라우팅 지연시간** | **< 50 마이크로초** 패스트패스 | 15 ~ 45 밀리초 |
| **메모리 사용량** | **~18 MB** (유휴 상태) | 120 ~ 350 MB |
| **SSE 스트리밍 할당** | 출력 버퍼 직접 쓰기 (`0`회 중간 할당) | 빈번한 문자열 결합 및 JSON 파싱 |
| **RTK 토큰 압축** | 네이티브 차분 압축 (20~40% 절감) | 압축 없음 |
| **추론 체인 표준화**| Claude 3.7, DeepSeek R1, o1/o3/o4 완벽 지원 | `<think>` 블록 누락 빈번 |
| **대시보드 UI** | Leptos WebAssembly SPA (35개 언어) | 무거운 Webpack/React 번들 |

---

## ⚡ 빠른 시작

### 1. NullRouter 실행

```bash
git clone https://github.com/nullrouter/nullrouter.git
cd nullrouter
cargo run --release
```

또는 스크립트 실행:
```bash
./run.sh
```

주요 주소:
- **API 엔드포인트**: `http://localhost:20128/v1`
- **대시보드**: `http://localhost:20128/dashboard`
- **채팅 플레이그라운드**: `http://localhost:20128/dashboard/chat`

---

### 2. 클라이언트 도구 설정

#### 🔹 Claude Code
```bash
export ANTHROPIC_BASE_URL="http://localhost:20128"
export ANTHROPIC_API_KEY="nullrouter-local"
claude
```

#### 🔹 Cursor
1. **Cursor Settings** ➔ **Models**로 이동합니다.
2. **OpenAI API Key**를 켜고 `nullrouter-local`을 입력합니다.
3. **OpenAI Base URL**을 재정의합니다: `http://localhost:20128/v1`.
4. 사용할 모델 이름을 추가합니다 (`claude-3-7-sonnet`, `deepseek-r1`, `auto`).

---

## 📖 GitHub Wiki 전체 가이드

자세한 기술 문서는 [NullRouter GitHub Wiki](../wiki/Home.md)에서 확인하세요:
- 📘 [설치 및 실행 가이드](../wiki/Getting-Started.md)
- 🏗️ [Pingora 코어 및 제로카피 아키텍처](../wiki/Architecture-and-Performance.md)
- 💻 [IDE 및 CLI 클라이언트 연동](../wiki/Client-Integrations.md)
- 🔑 [40개 이상 제공자 및 다중 계정 설정](../wiki/Provider-Configuration.md)
- 🩺 [상세 문제 해결 및 진단 플레이북](../wiki/Diagnostics-and-Troubleshooting.md)

---

## 📄 라이선스

NullRouter는 [MIT 라이선스](LICENSE)를 따르는 오픈 소스 소프트웨어입니다.
