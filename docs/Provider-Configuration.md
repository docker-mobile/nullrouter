# 🔑 Provider Configuration & Multi-Account Setup

NullRouter unifies over 40 AI providers under a single normalized protocol. This guide explains how to connect and configure providers across Subscription, Cheap, Free, and Local tiers.

---

## 🗂️ Provider Tiers Overview

NullRouter prioritizes routes across three distinct tiers:

1. **Tier 1: Subscription Providers**
   - High-cost or fixed monthly billing (e.g. Anthropic Pro/Team, OpenAI Team/Plus, GitHub Copilot).
   - Use these first until quota reset periods or rate limits hit.
2. **Tier 2: Ultra-Cheap Pay-as-you-go**
   - Providers offering high-intelligence frontier models at $0.10–$0.60 per million tokens (e.g. DeepSeek R1/V3.2, GLM-4.7, MiniMax, Groq).
   - Activates automatically when Tier 1 reaches quota limits.
3. **Tier 3: Free & Local Models**
   - Zero-cost providers (e.g. Kiro AI, OpenCode Free, Google Vertex free credits) or local engines (Ollama, vLLM, LM Studio).
   - Guarantees your coding environment never stops, even with empty API balances.

---

## ⚙️ Connecting Providers via the Dashboard

1. Navigate to `http://localhost:20128/dashboard`.
2. Select **Providers** in the sidebar.
3. Find your provider and click **Connect**.
4. Enter your API Key or OAuth credentials.
5. Click **Save Connection**.

Alternatively, edit `./nullrouter-state.json` directly.

---

## 📋 Configuration Details by Provider

### 1. Anthropic (Claude 5 / 4.6)
- **Supported Models**: `claude-opus-5` (2026-07-24), `claude-sonnet-4-6` (2026-02-17), `claude-haiku-4-5` (2025-10-15), `claude-fable-5.1` (2026-09-01) — source: `models.dev` `anthropic` (see `https://models.dev/api.json`).
- **Hybrid Extended Thinking**: Automatically supported. When requests omit `thinking`, NullRouter defaults to medium budget (8,192 tokens) and elevates `max_tokens` appropriately.
- **Prompt Caching**: Automatically enabled; cached blocks reuse context across requests.

### 2. DeepSeek (V4)
- **Supported Models**: `deepseek-v4-pro` (2026-08-12), `deepseek-v4-flash` (2026-09-10), `deepseek-v3.2` (2025-12-01) — per `models.dev` `deepseek`.
- **Prompt Caching Normalization**: DeepSeek's `prompt_cache_hit_tokens` and `prompt_cache_miss_tokens` are translated into standard `cache_read_input_tokens` and `cache_creation_input_tokens`.
- **Native `<think>` Streaming**: Reasoning tokens are streamed cleanly without polluting downstream tool call inputs.

### 3. OpenAI (GPT-6 / 5.6 & Reasoning)
- **Supported Models**: `gpt-6-astra` (2026-09-04), `gpt-5.6` (2026-07-09), `gpt-5.4-mini` (2026-03-17), `o3`, `o4-mini` — per `models.dev` `openai`.
- **Reasoning Effort Translation**: Seamlessly maps `reasoning_effort: "low" | "medium" | "high"` to upstream models.

### 4. Google Gemini (3.8)
- **Supported Models**: `gemini-3.8-flash` (2026-09-02), `gemini-3.7-flash` (2026-08-13), `gemini-3.6-flash` (2026-07-21) — per `models.dev` `google`.
- **Free Quota**: Gemini's free tier (15 requests/minute) is handled gracefully with automatic retry on 429.

### 5. Kiro AI (Free Tier)
- **Features**: Free monthly tier providing access to Claude Sonnet 4.6, GLM-5.2, and MiniMax M3 — proxied free models tracked via `models.dev`.
- **Setup**: One-click connect in Dashboard without manual token creation.

### 6. OpenCode Free (Zero Auth)
- **Features**: Free open-source model proxy (Qwen3.8 Coder, Llama 4).
- **Setup**: Toggle ON in Dashboard; no API key required.

### 7. Local Models (Ollama, vLLM, LM Studio)
- **Ollama**:
  - Default Endpoint: `http://localhost:11434/v1`
  - Recommended Models: `qwen3.8-coder:30b`, `deepseek-v4:32b`, `llama4:maverick`.
- **vLLM**:
  - Point the connection URL to `http://localhost:8000/v1`.
- **LM Studio**:
  - Point the connection URL to `http://localhost:1234/v1`.

---

## 🔄 Multi-Account Pooling & Key Rotation

You can register **multiple accounts** for the same provider to multiply your rate limits:

```json
{
  "connections": [
    {
      "id": "anthropic-personal",
      "provider": "anthropic",
      "api_key": "sk-ant-api03-xxxx",
      "weight": 1,
      "quota_limit": 5000000
    },
    {
      "id": "anthropic-work",
      "provider": "anthropic",
      "api_key": "sk-ant-api03-yyyy",
      "weight": 1,
      "quota_limit": 10000000
    }
  ]
}
```

- **Round-Robin Balancing**: Requests are distributed across healthy keys.
- **Circuit Breaking**: If one key hits a 429 rate limit, it is temporarily marked down and requests are automatically re-routed to the sibling key.
