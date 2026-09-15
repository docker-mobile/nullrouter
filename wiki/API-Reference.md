# 📑 REST & Streaming API Reference

NullRouter provides both OpenAI-compatible and Anthropic-compatible HTTP/SSE endpoints.

---

## 🌐 Base URL Coordinates
- **OpenAI Standard**: `http://localhost:20128/v1`
- **Anthropic Standard**: `http://localhost:20128`
- **Management & Metrics**: `http://localhost:20128/api`

---

## 1. OpenAI Chat Completions

### `POST /v1/chat/completions`

Creates a model completion for a given conversation. Supports standard JSON responses and Server-Sent Event (SSE) streams.

#### Request Headers:
```http
Content-Type: application/json
Authorization: Bearer nullrouter-local
```

#### Request Body Schema:
```json
{
  "model": "auto",
  "messages": [
    {
      "role": "system",
      "content": "You are an expert Rust systems programmer."
    },
    {
      "role": "user",
      "content": "Explain Pingora's fast-path dispatch mechanism."
    }
  ],
  "temperature": 0.7,
  "max_tokens": 4096,
  "stream": true,
  "reasoning_effort": "medium"
}
```

#### Streaming SSE Response Event:
```http
HTTP/1.1 200 OK
Content-Type: text/event-stream
Cache-Control: no-cache
Connection: keep-alive
server: pingora

data: {"id":"chatcmpl-null-01","object":"chat.completion.chunk","created":1789418000,"model":"claude-opus-5","choices":[{"index":0,"delta":{"role":"assistant","content":"Pingora"},"finish_reason":null}]}

data: {"id":"chatcmpl-null-01","object":"chat.completion.chunk","created":1789418000,"model":"claude-opus-5","choices":[{"index":0,"delta":{"content":" uses"},"finish_reason":null}]}

data: [DONE]
```

---

## 2. Model Discovery

### `GET /v1/models`

Returns the aggregated catalog of all active upstream models, combo aliases, and local models.

#### Response:
```json
{
  "object": "list",
  "data": [
    {
      "id": "auto",
      "object": "model",
      "created": 1789400000,
      "owned_by": "nullrouter-combo"
    },
    {
      "id": "claude-opus-5",
      "object": "model",
      "created": 1789400000,
      "owned_by": "anthropic"
    },
    {
      "id": "deepseek-v4-pro",
      "object": "model",
      "created": 1789400000,
      "owned_by": "deepseek"
    }
  ]
}
```

---

## 3. Anthropic Messages API

### `POST /v1/messages` (or `/messages`)

Native Anthropic schema support for Claude Code and Anthropic SDKs.

#### Request Schema (per `models.dev` `anthropic` — e.g. `claude-opus-5` 2026-07-24):
```json
{
  "model": "claude-opus-5",
  "max_tokens": 8192,
  "thinking": {
    "type": "enabled",
    "budget_tokens": 4096
  },
  "messages": [
    {
      "role": "user",
      "content": "Refactor this function into zero-copy Rust."
    }
  ]
}
```

---

## 4. Operational & Management Endpoints

### `GET /api/health`
Returns the operational health of the Pingora gateway and Actix microservices:
```json
{
  "status": "ok",
  "gateway": "healthy",
  "uptime_seconds": 86400,
  "active_connections": 12
}
```

### `GET /api/metrics`
Prometheus-compatible scraping endpoint for observability:
```
# HELP nullrouter_requests_total Total number of HTTP requests handled
# TYPE nullrouter_requests_total counter
nullrouter_requests_total{route="/v1/chat/completions",status="200"} 45210
# HELP nullrouter_dispatch_latency_microseconds Gateway dispatch latency
# TYPE nullrouter_dispatch_latency_microseconds histogram
nullrouter_dispatch_latency_microseconds_bucket{le="50"} 44900
```
