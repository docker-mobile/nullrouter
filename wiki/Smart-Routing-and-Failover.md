# 🔀 Smart Routing & Failover Strategies

NullRouter's dynamic routing engine ensures that coding sessions never fail due to upstream rate limits, provider outages, or quota exhaustion.

---

## 🎯 Combo Models & Model Aliasing

Instead of hardcoding a specific provider model in your editor, NullRouter lets you use **Combo Aliases**. These are virtual model identifiers that point to prioritized fallback chains.

### Default Built-in Combos

| Combo Name | Target Models (in priority order) | Typical Use Case |
| :--- | :--- | :--- |
| `auto` | `claude-3-7-sonnet` ➔ `deepseek-r1` ➔ `kiro-claude` | Best general coding agent setup |
| `fast` | `gpt-4o-mini` ➔ `gemini-2.0-flash` ➔ `groq-llama-3.3-70b` | Autocompletion, quick inline edits |
| `smart` | `o3-mini` ➔ `claude-3-7-sonnet` ➔ `deepseek-r1` | Complex architectural reasoning |
| `coder` | `qwen-2.5-coder-32b` ➔ `codestral` ➔ `deepseek-v3` | Dedicated code generation & test writing |

### Customizing Combos in the Dashboard
1. Go to **Dashboard** ➔ **Combos**.
2. Click **Create New Combo**.
3. Specify the name (e.g. `my-team-combo`).
4. Drag and drop providers to set the priority order.
5. Save changes. Changes take effect immediately without restarting the gateway.

---

## 🛡️ Failover Triggers & Circuit Breaking

When a client sends a completion request, `runtime-actix` manages the upstream request lifecycle:

```
[Client Request]
       │
       ▼
[Try Primary Provider]
       │
       ├──► 200 OK (Stream begins) ──────────► [Stream to Client]
       │
       ├──► 429 Too Many Requests ──┐
       ├──► 500 / 502 / 503 Outage ──┼───────► [Switch to Next Provider in Chain]
       └──► Timeout (>15s without data) ┘
```

### Transparent In-Flight Failover
If an upstream provider returns a 429 or 5xx status code **before** the first SSE token is yielded to the client, NullRouter immediately cancels the dead connection and dispatches the exact same payload to the next provider in the chain.
- The downstream IDE receives a continuous, unbroken response.
- No error dialogs appear in VS Code or Cursor.

### Circuit Breaker States
- **Closed (Healthy)**: All requests pass through normally.
- **Open (Unhealthy)**: After 3 consecutive network failures or a 429 rate limit with `retry-after`, the provider is locked out for 60 seconds (or the provider's `retry-after` header duration).
- **Half-Open (Testing)**: A single probe request tests recovery; on success, the provider is restored.

---

## 📊 Quota Tracking & Billing Cycle Resets

For subscription-based providers (like Claude Code or OpenAI Team):
- NullRouter tracks token consumption and approximate costs in real time.
- Users can set monthly token thresholds (e.g. 10,000,000 tokens).
- When a threshold is reached, NullRouter automatically demotes the subscription provider and routes traffic to the cheap tier until the configured billing cycle reset date.
