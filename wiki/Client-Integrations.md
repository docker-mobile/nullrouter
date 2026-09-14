# 🔌 Comprehensive Client Integration Guide

NullRouter presents standard, 100% compliant OpenAI and Anthropic HTTP/SSE endpoints. It can be integrated into any coding agent, editor, or IDE in seconds.

---

## 🧭 General Connection Coordinates

| Parameter | Value | Notes |
| :--- | :--- | :--- |
| **OpenAI Base URL** | `http://localhost:20128/v1` | Used for standard OpenAI clients |
| **Anthropic Base URL**| `http://localhost:20128` | Used for Claude-native SDKs |
| **API Key** | `nullrouter-local` | Any non-empty string is accepted locally |
| **Universal Model** | `auto` | Automatically routes to best available model |

---

## 1. 🤖 Claude Code (Anthropic CLI)

Claude Code communicates with Anthropic's Messages API natively. NullRouter translates requests dynamically or routes them directly to Anthropic or compatible reasoning models.

Add these lines to your shell profile (`~/.bashrc` or `~/.zshrc`):

```bash
export ANTHROPIC_BASE_URL="http://localhost:20128"
export ANTHROPIC_API_KEY="nullrouter-local"
```

Then run:
```bash
claude
```

NullRouter will intercept Claude Code's large tool outputs (`git diff`, `bash`, `grep`), apply RTK token compression, and preserve Claude 3.7 extended thinking tokens automatically.

---

## 2. ⚡ Cursor IDE

1. Open Cursor and press `Cmd + Shift + J` (macOS) or `Ctrl + Shift + J` (Windows/Linux) to open **Settings**.
2. Navigate to **Models**.
3. Toggle ON **OpenAI API Key** and enter `nullrouter-local`.
4. Check **Override OpenAI Base URL** and enter:
   ```
   http://localhost:20128/v1
   ```
5. In the model list, add:
   - `auto` (Smart combo routing)
   - `claude-3-7-sonnet`
   - `deepseek-r1`
   - `gpt-4o`
6. Click **Save**.

---

## 3. 🧩 Cline (VS Code Extension)

1. Open VS Code with the Cline extension installed.
2. Click the gear icon in the Cline panel to open Settings.
3. Set **API Provider** to **OpenAI Compatible**.
4. Configure fields:
   - **Base URL**: `http://localhost:20128/v1`
   - **API Key**: `nullrouter-local`
   - **Model ID**: `auto` (or `claude-3-7-sonnet`)
5. Click **Done**.

---

## 4. 🦘 Roo Code (VS Code Extension)

1. Open Roo Code Settings in VS Code.
2. Select Provider: **OpenAI Compatible** (or **Anthropic**).
3. If using OpenAI Compatible:
   - **Base URL**: `http://localhost:20128/v1`
   - **API Key**: `nullrouter-local`
   - **Model**: `auto`
4. If using Anthropic:
   - **Base URL**: `http://localhost:20128`
   - **API Key**: `nullrouter-local`
5. Enable **Streaming** and **Thinking Budgets** if prompted.

---

## 5. 💻 OpenAI Codex CLI

Set the environment variables before starting Codex:

```bash
export OPENAI_BASE_URL="http://localhost:20128/v1"
export OPENAI_API_KEY="nullrouter-local"
codex
```

---

## 6. 🔄 Continue.dev

Edit your Continue configuration file at `~/.continue/config.json`:

```json
{
  "models": [
    {
      "title": "NullRouter Smart Route",
      "provider": "openai",
      "model": "auto",
      "apiBase": "http://localhost:20128/v1",
      "apiKey": "nullrouter-local"
    },
    {
      "title": "Claude 3.7 Thinking",
      "provider": "openai",
      "model": "claude-3-7-sonnet",
      "apiBase": "http://localhost:20128/v1",
      "apiKey": "nullrouter-local"
    },
    {
      "title": "DeepSeek R1",
      "provider": "openai",
      "model": "deepseek-r1",
      "apiBase": "http://localhost:20128/v1",
      "apiKey": "nullrouter-local"
    }
  ]
}
```

---

## 7. 🌊 Windsurf (Codeium Cascade)

1. Open **Windsurf Settings** ➔ **AI Model Configuration**.
2. Add a custom OpenAI endpoint:
   - **API Base**: `http://localhost:20128/v1`
   - **API Key**: `nullrouter-local`
   - **Model ID**: `auto`

---

## 8. 🛠️ Aider CLI

Launch Aider with:

```bash
aider \
  --openai-api-base http://localhost:20128/v1 \
  --openai-api-key nullrouter-local \
  --model auto
```

For reasoning models with architect mode:
```bash
aider \
  --openai-api-base http://localhost:20128/v1 \
  --openai-api-key nullrouter-local \
  --model claude-3-7-sonnet \
  --editor-model deepseek-r1
```

---

## 9. ⌨️ Neovim (Avante.nvim & CodeCompanion)

### Avante.nvim (`lazy.nvim` config)
```lua
{
  "yetone/avante.nvim",
  opts = {
    provider = "openai",
    openai = {
      endpoint = "http://localhost:20128/v1",
      model = "auto",
      api_key_name = "NULLROUTER_API_KEY",
    },
  },
}
```

Set in your shell:
```bash
export NULLROUTER_API_KEY="nullrouter-local"
```
