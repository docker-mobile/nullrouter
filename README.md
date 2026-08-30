# nullrouter

**One local port. 117 AI providers. Any client format, any provider format.**

[![Rust](https://img.shields.io/badge/rust-1.88%2B-orange?logo=rust)](rust-toolchain.toml)
[![Edition](https://img.shields.io/badge/edition-2024-blue?logo=rust)](Cargo.toml)
[![License](https://img.shields.io/badge/license-MIT-green)](Cargo.toml)
[![Tests](https://img.shields.io/badge/tests-1117-brightgreen)](#development)

nullrouter is a Rust microservice port of [9Router](https://github.com/decolua/9router): a local
gateway that puts every AI provider you have credentials for behind a single OpenAI-compatible
endpoint — and translates between wire formats in both directions as it goes.

Point Claude Code at it and drive a GPT model. Point an OpenAI SDK at it and drive Claude. Point a
Gemini SDK at it and drive Groq. The client never learns that the provider spoke a different
protocol.

```bash
# A Claude-format request…
curl -X POST http://127.0.0.1:20128/v1/messages \
  -H 'content-type: application/json' \
  -d '{"model":"openai/gpt-5","max_tokens":1024,
       "messages":[{"role":"user","content":"hello"}]}'

# …answered by OpenAI, returned as native Claude SSE events.
```

---

## Contents

- [Why](#why)
- [Quick start](#quick-start)
- [Architecture](#architecture)
- [Provider execution](#provider-execution)
- [The registry](#the-registry)
- [Security model](#security-model)
- [Dashboard](#dashboard)
- [Migrating from 9Router](#migrating-from-9router)
- [Configuration](#configuration)
- [What is deliberately not implemented](#what-is-deliberately-not-implemented)
- [Development](#development)

---

## Why

Every AI client speaks its own dialect. OpenAI Chat Completions, the newer OpenAI Responses API,
Anthropic Messages, and Google Gemini all model the same conversation differently — tool calls,
system prompts, image blocks, streaming envelopes, reasoning traces. Switching a tool from one
provider to another usually means switching tools.

nullrouter sits in the middle and does the translation, incrementally, on the streaming path:

| You point this at nullrouter | …and it can drive |
|---|---|
| OpenAI Chat SDK (`/v1/chat/completions`) | OpenAI · Anthropic · Gemini · 100+ more |
| OpenAI Responses SDK (`/v1/responses`) | any of the above, with real lifecycle events |
| Anthropic SDK / Claude Code (`/v1/messages`) | any of the above, as native Claude SSE |
| Gemini SDK (`/v1beta/models/{model}:generateContent`) | any of the above |
| Clients posting to `/v1/api/chat` | any of the above, format detected from the body |

Plus embeddings, image generation, text-to-speech, transcription, web search, and web fetch, each
dispatched to whichever provider in the registry actually exposes that service.

## Quick start

**Prerequisites:** Rust stable (1.88+, edition 2024), plus
[`wasm-bindgen-cli`](https://crates.io/crates/wasm-bindgen-cli) and the
`wasm32-unknown-unknown` target for the dashboard.

**1. Build the dashboard WASM bundle once.** This is not optional — the dashboard,
the sign-in screen, and the OAuth callback are all served by it, so without it every
page is an empty shell:

```bash
rustup target add wasm32-unknown-unknown
cargo build -p nullrouter-dashboard-wasm --lib --target wasm32-unknown-unknown --release
wasm-bindgen --target web \
  --out-dir services/dashboard-actix/static/pkg \
  --out-name dashboard_leptos \
  target/wasm32-unknown-unknown/release/nullrouter_dashboard_wasm.wasm
```

**2. Start the services.** Bring up `nullrouter-state` first — the runtime and API read credentials
and usage from it. Each of these blocks, so use separate terminals or your process manager of choice:

```bash
NULLROUTER_STATE_FILE=./nullrouter-state.json cargo run -p nullrouter-state
cargo run -p nullrouter-runtime
cargo run -p nullrouter-api
cargo run -p nullrouter-events
cargo run -p nullrouter-catalog
cargo run -p nullrouter-auth
cargo run -p nullrouter-dashboard-host
cargo run -p nullrouter-gateway
```

> **Set `NULLROUTER_STATE_FILE`.** Without it the state service runs **entirely in memory** and every
> provider connection, key, and usage record is lost when it exits.

**3. Open the dashboard** at <http://127.0.0.1:20128/dashboard/endpoint> and sign in. The default
password is `123456` unless you set `INITIAL_PASSWORD` or `NULLROUTER_AUTH_PASSWORD_HASH` — change it
before exposing this anywhere.

**4. Add a provider connection** in the dashboard, then call through the one public port:

```bash
curl -X POST http://127.0.0.1:20128/v1/chat/completions \
  -H 'content-type: application/json' \
  -d '{"model":"openai/gpt-5","messages":[{"role":"user","content":"hello"}],"stream":true}'
```

Models are addressed as `provider/model` (`anthropic/claude-sonnet-4.5`, `groq/llama-3.3-70b`), by
provider alias (`cc/claude-sonnet-4.5`), or by a combo name you defined in the dashboard.

## Architecture

Eight binaries plus a WASM dashboard. **Only the Pingora gateway listens publicly**; every other
service refuses to start against a non-loopback upstream.

```
                    ┌──────────────────────────────────────┐
   client ─────────▶│  nullrouter-gateway   :20128         │  ← the only public port
   (curl, SDK,      │  Pingora · routing · auth policy     │
    Claude Code)    └───┬──────────────────────────────────┘
                        │  path-based routing
        ┌───────────────┼───────────────┬──────────────┬─────────────┐
        ▼               ▼               ▼              ▼             ▼
  ┌───────────┐   ┌───────────┐   ┌──────────┐  ┌──────────┐  ┌──────────┐
  │  runtime  │   │    api    │   │  events  │  │ catalog  │  │   auth   │
  │   :20132  │   │   :20129  │   │  :20133  │  │  :20131  │  │  :20135  │
  │ /v1 /v1beta│  │  /api/*   │   │ SSE feeds│  │inventory │  │ sessions │
  └─────┬─────┘   └─────┬─────┘   └────┬─────┘  └──────────┘  │ OIDC/SAML│
        │               │              │                      └────┬─────┘
        │  credentials  │  aggregates  │  usage snapshots          │
        └───────────────┴──────┬───────┴───────────────────────────┘
                               ▼
                        ┌─────────────┐          ┌────────────────────┐
                        │    state    │          │  dashboard-host    │
                        │    :20134   │          │      :20130        │
                        │ credentials │          │  Leptos CSR / WASM │
                        │ usage · cfg │          └────────────────────┘
                        └──────┬──────┘
                               │  /internal/* — refused at the gateway
                               ▼
                     unredacted secrets to runtime only
```

| Service | Binary | Port | Role |
|---|---|---|---|
| `services/gateway-pingora` | `nullrouter-gateway` | **20128** | The one public port. Path routing, access policy, header sanitising |
| `services/runtime-actix` | `nullrouter-runtime` | 20132 | **Provider execution.** `/v1`, `/v1beta`, translation, streaming, fallback |
| `services/api-actix` | `nullrouter-api` | 20129 | Dashboard control API, usage projections, migration |
| `services/state-actix` | `nullrouter-state` | 20134 | Persistence: connections, credentials, combos, pools, usage |
| `services/auth-actix` | `nullrouter-auth` | 20135 | Password sessions, lockout, OIDC (PKCE), SAML metadata |
| `services/events-actix` | `nullrouter-events` | 20133 | SSE: live usage, console logs, MCP transport |
| `services/catalog-actix` | `nullrouter-catalog` | 20131 | Route and provider inventory for the dashboard |
| `services/dashboard-actix` | `nullrouter-dashboard-host` | 20130 | Static + WASM host, 34 locale bundles |
| `apps/dashboard-leptos` | `nullrouter-dashboard-wasm` | — | Leptos CSR dashboard |

### Shared crates

| Crate | What it owns |
|---|---|
| `crates/providers` | The registry: 117 providers, 850 models, 685 per-model capability rows, 59 providers with non-chat service endpoints. Wire-format detection, model/alias resolution |
| `crates/translate` | Bidirectional translation across OpenAI Chat, OpenAI Responses, Claude, and Gemini — request and incremental streaming response |
| `crates/execute` | Provider HTTP: auth descriptors, URL fallback, per-status retry, SSE piping, error classification, per-connection outbound proxy, OAuth refresh grants |
| `crates/pxpipe` | The PXPIPE token saver: npm install management, the Node transform worker, the eligibility gate, and the event log the dashboard aggregates |
| `crates/contracts` | Typed response contracts shared by services and the WASM dashboard |

### How the gateway routes

| Path | Goes to |
|---|---|
| `/v1/*`, `/v1beta/*`, `/api/v1/*`, `/api/v1beta/*` | runtime |
| `/api/usage/stream`, `/api/translator/console-logs/stream`, `/api/mcp/*` | events |
| `/api/auth/*` | auth |
| `/api/catalog/*`, `/api/state/*` | catalog |
| `/api/keys`, `/api/providers`, `/api/combos`, `/api/proxy-pools`, `/api/provider-nodes`, `/api/settings` (collection + item) | state (direct CRUD) |
| everything else under `/api/*` | api |
| everything else | dashboard host |

## Provider execution

### Format translation

Translation pivots through OpenAI (`source → openai → target`), the same way upstream does. Both
directions are ported for `openai ↔ claude` and `openai ↔ gemini`; the Gemini response path also
covers `gemini-cli`, `vertex`, and `antigravity`, which share Gemini's response shape.

Source format is detected from the request path first (`/v1/messages` → Claude, `/v1/responses` →
Responses API), then from the body shape. The body probe reproduces upstream's precedence exactly,
including its JS truthiness quirks — `logprobs: null` counts as present and forces OpenAI, but
`user: ""` is falsy and does not.

### The Responses API is real

`/v1/responses` speaks its actual protocol, not reshaped chat chunks:

- `input[]` and `instructions` are regrouped into chat turns on the way out.
- The reply is a sequence of named lifecycle events — `response.created`,
  `response.output_text.delta`, `response.completed`, … — with monotonic `sequence_number`.
- Every item that gets opened is explicitly closed, and the stream is finalised even when the
  upstream provider just stops. A client that waits for `response.completed` will get it.

### Streaming

Frames reach the client as each upstream chunk is parsed, so time-to-first-token tracks the
provider's own latency rather than the full completion time. The pipe runs through a bounded
64-frame channel: memory stays flat regardless of response length, and a slow client applies
backpressure to the upstream read instead of dropping frames. Frames are never dropped on a full
channel — a truncated frame would corrupt the client's JSON parse.

Every service that streams sets `TCP_NODELAY` on accepted sockets. Without it a streamed response
cost a client that reuses connections — which is every real client — roughly **twice** its actual
latency: the headers go out as one small write, and once the connection has left Linux's initial
quickack mode, Nagle holds the body until the client's delayed-ACK timer fires. It was worth 40 ms
per streamed response and no functional test could see it, since the bytes were all correct and only
late. See [BENCHMARKS.md](BENCHMARKS.md).

A provider that only streams (`forceStream` in the registry) is still called with `stream=true` when
the client asked for JSON; the stream is then collapsed back into a single body.

### Account fallback

On a retryable failure the runtime walks to the next account for that provider, up to 10 attempts:

- Per-model and account-level cooldowns, stored as expiry timestamps on the connection.
- Exponential backoff levels for quota errors.
- A successful call clears that account's prior cooldown.
- When every account is cooling down, the client gets the **last real upstream error** plus a
  `Retry-After` header — not a generic 503.
- Round-robin selection is sticky, ordered by last-used time and consecutive-use count.

### Combo fallback

A combo is one name fronting several models. Account fallback above walks *accounts* for one model;
this walks *models*. Both apply: each model exhausts its own accounts before the combo advances.

- **`fallback`** (default) tries the models in configured order.
- **`round-robin`** advances the starting model each request, wrapping — so every model stays a
  fallback for the others rather than only the ones after it. `comboStickyRoundRobinLimit` (default 1)
  holds one model for that many requests before moving on.
- The **last** model owns the client-visible outcome. If nothing answered, you get that model's real
  status and message, not a synthesised "all models unavailable".
- A model whose provider needs an unported executor is stepped past rather than ending the combo, so
  one `ollama` entry does not disable the whole combo. A *single-model* request for that provider still
  gets its explicit 501 naming the protocol.
- Rotation is per combo, and a combo whose model list was edited starts over: a cursor recorded against
  a different list points at an arbitrary model.

- **`fusion`** asks every model at once and has a judge write one answer from all of them.

### Fusion combos

A fusion combo fans the prompt out to the whole panel in parallel, then a judge model
writes the final reply from their answers:

- Panel calls are forced **non-streaming with tools stripped**, and tool turns in the
  history are flattened to prose. A panel model that could still emit `tool_calls`
  would hand the judge a half-finished turn, and the client never sees panel output.
- The judge keeps the client's **original stream flag and tools**, so streaming and
  downstream tool use still work.
- Panel answers reach the judge as **"Source 1", "Source 2"** — never model names, so
  it weighs substance rather than vendor reputation.
- Collection is **quorum-graced**: once two answers arrive the rest get 8s, capped by
  a 90s hard timeout, so the slowest model does not set the request's latency.
- Degrades honestly: **zero** answers is a 503, and **exactly one** is returned
  directly rather than paying for a judge call to paraphrase a single response.
- Every panel call is recorded in usage, so a fusion combo does not under-report by
  the size of its panel.

### Per-combo strategy overrides

`comboStrategies[name]` on `/api/settings` overrides the global `comboStrategy` for one combo. Global
`fallback` with `{"panel": {"fallbackStrategy": "fusion"}}` makes exactly that combo fan out; the
reverse turns fusion off for one combo while leaving it on everywhere else. An entry may also carry
fusion tuning (`minPanel`, `stragglerGraceMs`, `panelHardTimeoutMs`), and anything it leaves unset
keeps the default rather than being reset.

Two details that are easy to get wrong and are pinned by tests: a write that names `comboStrategies`
**replaces** the map, because upstream's dashboard prunes an entry when a combo returns to the default
and a merge would make an override impossible to remove — while a settings write that does not mention
the map leaves every override alone. And an unrecognised strategy name degrades to the global rather
than failing the request.

### Per-model output ceilings

`max_tokens` is clamped to the model's real ceiling from the capability table (685 per-model rows),
not a conservative global default. A model with a 128k output limit is not silently truncated at
64000.

### Reasoning is translated too

Every vendor spells "think harder" differently, and a reasoning field a provider does not recognise is
either ignored or a 400 — both silent from the caller's side. So thinking intent is read out of
whichever dialect it arrived in and re-emitted in the target provider's own shape, across **12 wire
formats** covering **568 of the 685** models in the capability table:

| Format | Wire shape | Models |
|---|---|---|
| `openai` | `reasoning_effort: "high"` | 139 |
| `qwen` | `enable_thinking` + `thinking_budget` | 71 |
| `zai` | `enable_thinking` (ignores Anthropic's disable) | 59 |
| `kimi` | Anthropic disable + OpenAI effort | 51 |
| `deepseek` | `thinking` + effort collapsed to `high`/`max` | 46 |
| `claude-budget` | `thinking: {budget_tokens}` | 45 |
| `claude-adaptive` | `thinking: {type:"adaptive"}` + `output_config.effort` | 43 |
| `minimax` | `thinking: {type:"adaptive"}` | 40 |
| `gemini-level` | `thinkingConfig.thinkingLevel` | 40 |
| `gemini-budget` | `thinkingConfig.thinkingBudget` | 14 |
| `hunyuan` | Anthropic budget shape | 4 |
| `step` | effort capped at `high` | 3 |

A `reasoning_effort: "high"` aimed at an Anthropic budget model becomes
`thinking: {type:"enabled", budget_tokens:24576}`; the reverse becomes `reasoning_effort: "high"`. The
source spelling is always removed, so a request never carries two conflicting instructions.

Details that are easy to get wrong and are pinned by tests:

- **A model that cannot disable thinking** gets minimal effort instead of an "off" it would reject —
  otherwise you are billed for full-price reasoning while believing it is switched off.
- **Gemini draws thinking tokens from the output budget**, so `maxOutputTokens` is raised to the floor
  that budget needs (never above the model's own ceiling). Left alone, reasoning eats the ceiling and
  truncates the visible answer.
- **gemini-cli and antigravity wrap the request in an envelope**; the config *inside* it is written.
  Writing the top-level one sets a field the provider never reads.
- **Budgets are clamped to the model's stated range** — a 999999-token budget on a 24576-cap model is
  clamped, not rejected.
- **A non-reasoning model has thinking stripped entirely**, since an unrecognised field is a 400 on
  several providers.
- **Nothing is invented.** A request that says nothing about reasoning is passed through untouched.

Per-request effort can also be pinned on the model id: `openai/gpt-5(high)`, `(8192)`, `(none)`, or
`(auto)`. The suffix wins over the body and never reaches the provider.

### Non-chat services

These dispatch to each provider's own service-specific endpoint from the registry, not to a chat
endpoint:

| Route | Service |
|---|---|
| `/v1/embeddings` | embeddings (34 embedding models) |
| `/v1/images/generations` | image generation (64 image models) |
| `/v1/audio/speech`, `/v1/audio/voices` | text-to-speech (29 models) |
| `/v1/audio/transcriptions` | transcription (22 STT models) |
| `/v1/search` | web search |
| `/v1/web/fetch` | web fetch |

On these routes a bare token names a *provider*, not a model — `{"provider":"tavily"}` on `/v1/search`
routes to Tavily rather than being inferred as a model alias.

### Also on the runtime

`/v1/models` (registry-backed, scoped to your configured connections), `/v1/models/{kind}`,
`/v1/models/info`, `/v1/messages/count_tokens`, `/v1/responses/compact`, and `/v1/api/chat`.
Everything is mirrored under `/api/v1` and `/api/v1beta` for clients that expect that prefix.

### Usage

Every request is recorded — provider, model, connection, endpoint, status, prompt/completion/cached
tokens, latency, error — including failures. Non-streaming replies have their `usage` object read
directly from the body, in both OpenAI (`prompt_tokens`) and Claude (`input_tokens`) spellings, so
they do not record as zero-token requests.

Read it back through `/api/usage/stats`, `/api/usage/history`, `/api/usage/chart`, `/api/usage/logs`,
`/api/usage/request-logs`, `/api/usage/request-details`, `/api/usage/providers`, and
`/api/usage/{connectionId}`, or subscribe to `/api/usage/stream` for a live SSE feed. State keeps a
bounded ring of the 1000 most recent requests alongside running totals.

## The registry

Registry data is **generated from the upstream reference checkout**, not hand-transcribed, so
transports, auth descriptors, model tables, and per-model limits stay faithful. Generated from
upstream `v0.5.55` (`699edac3273e13d4744bc46f6082618f08560702`).

| | Count |
|---|---|
| Provider entries | 117 |
| Models | 850 |
| — LLM | 690 |
| — image | 64 |
| — embedding | 34 |
| — text-to-speech | 29 |
| — speech-to-text | 22 |
| — video | 9 |
| Per-model capability rows | 685 |
| Providers with non-chat service endpoints | 59 |
| Providers with OAuth descriptors | 19 |
| Providers with multiple format transports | 8 |

Each entry carries base URL (or an ordered list of fallback URLs tried on 429), wire format, auth
descriptor (combined or split API-key/OAuth headers), per-status retry policy, timeouts, regional
endpoints, and request-shaping quirks such as `preserveCacheControl` and `cloakToolsOnOauth`.

**Multi-transport providers are addressed in the client's own format.** Eight providers front more
than one endpoint on one host — `deepseek` answers OpenAI requests at `/chat/completions` and Claude
requests at `/anthropic/v1/messages`. A Claude client reaching one of those goes straight to the
Claude endpoint, with that endpoint's own headers and auth descriptor, and its body is **not
translated at all**. The selection is gated per model: `opencode-go` fronts several vendors and its
`kimi`/`glm` models serve `/chat/completions` only, so a Claude request for those is translated as
before rather than 404'd against `/messages`.

### What executes, and what refuses

The OpenAI-compatible, Anthropic-compatible, Gemini, and Vertex protocol families execute — the
large majority of the registry, including `openai`, `anthropic`, `gemini`, `groq`, `deepseek`,
`openrouter`, `mistral`, `cerebras`, `together`, `xai`, plus the dynamic `openai-compatible-*` and
`anthropic-compatible-*` families you define yourself.

`ollama` (including `ollama-local`, whose host comes from the connection), `gemini-cli` and
`commandcode` execute too. None of them needs a distinct executor: what they need is a request
envelope, a per-request header, or a URL suffix, and those are hooks on the shared path rather than
three more code paths.

Providers whose wire protocol needs genuine request signing or a binary protocol return an explicit
**501 naming the provider and its protocol**, rather than a plausible wrong answer:

`kiro` · `cursor` · `codex` · `antigravity` · `grok-web` · `perplexity-web`

A test asserts that more than 75% of registry entries with a transport remain executable.

## Security model

The boundaries below are pinned by tests, not just intent
(`services/gateway-pingora/tests/internal_boundary.rs` and neighbours).

**Only the gateway listens publicly.** `GatewayConfig::new` returns an error for any non-loopback
upstream, so a misconfiguration fails at startup instead of quietly exposing a service.

**`/internal/*` is refused at the gateway** from every peer, on every route, unconditionally. Those
endpoints hand **unredacted credentials** to the runtime — API keys, access tokens, refresh tokens —
and are safe only because that refusal holds.

**Stored secrets are stripped from every public API response.** `apiKey`, `accessToken`, and
`refreshToken` never leave through `/api/*`.

**Host-only routes require a loopback peer.** MITM control, MCP, tunnel control, headroom process
control, password reset, the OAuth auto-import helpers, and PXPIPE install/start return 403 to any
non-loopback caller even with a valid session. The PXPIPE pair is stricter than upstream, which allows
them from any authenticated dashboard session: they run `npm install pxpipe-proxy@latest`, whose
lifecycle scripts execute as the API service, and a session cookie taken from a browser on another
machine should not be able to install software on this one. The package name is fixed and never read
from the request.

**Forwarded headers are stripped and re-stamped.** `Forwarded`, `X-Forwarded-*`, `X-Real-IP`, and the
trusted `x-9r-*` pair are removed from inbound requests, then the real peer IP is stamped — a client
cannot spoof its own address to satisfy a loopback check.

**API keys are stored as digests only.** There is no code path that returns a stored key's plaintext.

**Access requirements by route:**

| Route class | Requirement |
|---|---|
| `/internal/*` | always **403** |
| `/`, `/login`, `/landing`, `/callback`, `/pkg/*`, `/providers/*`, `/assets/*`, `/favicon.svg`, `/api/health`, `/api/auth/*` | public |
| `/dashboard/*` | dashboard session → redirect to `/login` |
| `/api/*` (api, catalog, events, state) | API session → 401 |
| `/v1/*`, `/v1beta/*` | public **unless** managed API keys are enforced |

> **`/v1` is unauthenticated by default.** It is loopback-only, but anything that can reach the port
> can spend your provider credits. Start the gateway with `--require-api-key` (or
> `NULLROUTER_REQUIRE_API_KEY=true`) to require a managed key, and turn on `requireApiKey` in
> settings so the runtime enforces it too. The runtime checks the persisted setting on its own, at
> the last hop before a provider call, so a dashboard toggle is never silently ignored.

Keys are accepted as `Authorization: Bearer`, `x-api-key`, `x-goog-api-key`, or a `?key=` query
parameter.

### Sign-in

Password auth with bcrypt hashes, or a constant-time SHA-256 comparison for a plaintext configured
password. Sessions are HMAC-SHA256 tokens in an `auth_token` cookie with a 24-hour default TTL and 60
seconds of allowed clock skew. Per-IP lockout defaults to 5 failures in 15 minutes, locking for 15
minutes, bounded to 4096 tracked addresses.

**OIDC** is a full authorization-code flow with PKCE. A session is minted only after the `id_token`
verifies against the provider's published JWKS — an unverifiable signing algorithm is a failure, never
a skipped check. Every failure path redirects to `/login`.

**SAML** generates service-provider metadata and outbound `AuthnRequest`s. Consuming a `SAMLResponse`
is **deliberately refused**: verifying an assertion requires exclusive XML canonicalisation, and a
subtly wrong C14N implementation is an authentication bypass. `consume_response` validates everything
it can and then returns `VerificationUnavailable`. There is no code path in that module that produces
a session from a `SAMLResponse`.

### Outbound proxies

A provider connection can route through its own HTTP/SOCKS proxy (`connectionProxyEnabled` +
`connectionProxyUrl`, with `connectionNoProxy` bypasses). Proxied connections get their own client;
everything else shares a pooled one.

## Dashboard

**The frontend is entirely Rust.** One Leptos CSR/WASM bundle serves every screen —
the dashboard, the sign-in page, and the OAuth callback — and the actix host serves
only document shells that mount it. There is no application JavaScript: the shells
carry a two-line ES module that boots the bundle, and a `<noscript>` explaining why
the page is otherwise empty. A boundary test greps each shell's `<script>` blocks and
fails if fetch calls, event listeners, or redirect handling reappear there.

The sign-in shell deliberately has **no fallback `<form>`**. One would work without
WASM and would skip the redirect sanitiser and lockout countdown entirely; a silently
degraded sign-in is worse than a page that says what it needs.

Two consequences worth knowing:

- Every screen needs the bundle built (see [Quick start](#quick-start)). Without it
  you get empty shells, not a degraded-but-working dashboard.
  `/pkg/*` is public at the gateway, which is what lets the bundle load on `/login`
  before a session exists.
- The redirect sanitiser and the OAuth relay's origin restriction are unit-tested
  Rust rather than script in a string literal. Both handle untrusted input — a
  `?next=` from a link and an authorization code destined for another window — so
  they are tested directly: hostile `?next=` values (scheme-relative, `javascript:`,
  prefix-matching origins, backslash tricks) and a `postMessage` target that is never
  `"*"`.

The dashboard is served by `nullrouter-dashboard-host` in **35 locales** (34
bundles fetched at runtime, plus built-in English) covering Arabic, Bengali, Chinese (Simplified and
Traditional), Czech, Danish, Dutch, Farsi, Finnish, French, German, Greek, Hebrew, Hindi, Hungarian,
Indonesian, Italian, Japanese, Khmer, Korean, Norwegian, Polish, Portuguese (BR and PT), Romanian,
Russian, Spanish, Swedish, Tagalog, Thai, Turkish, Ukrainian, Urdu, and Vietnamese. An unknown or
hand-edited locale cookie degrades to English rather than breaking the app.

Sections: Endpoint & Key · Providers · Combos · Usage · Quota Tracker · Token Saver · CLI Tools ·
Embedding · Text to Image · Text To Speech · Speech To Text · Web Fetch & Search · Proxy Pools ·
Skills · MITM · Console Log · Translator · Settings · Pricing · Migrate.

Sections whose backend is not ported render as inactive rather than advertising a feature that does
not work. **Token Saver** is live: it installs and repairs the PXPIPE package, starts and stops the
transform, turns it on for the request path, and shows every attempt with the reason it went the way
it did. Its savings figures are labelled estimates everywhere they appear — they come from character
counts and image pixel areas, not from provider-billed usage, and the Usage page holds the recorded
cost of each request. When the service holding the transform is unreachable the panel reports the
running state as **Unknown** rather than as stopped, because a claim about a process it never reached
would send someone to fix the wrong thing.

## Migrating from 9Router

An existing 9Router install imports in place. Provider connections (including API keys and OAuth
tokens), combos, proxy pools, and routing settings carry over — including both fallback strategies and
their sticky limits, so an imported round-robin combo keeps rotating rather than quietly answering from
one model.

```bash
# Preview what would be imported, writing nothing.
curl -X POST http://127.0.0.1:20128/api/migrate/9router \
  -H 'content-type: application/json' -d '{"dryRun":true}'

# Import.
curl -X POST http://127.0.0.1:20128/api/migrate/9router
```

Requires a dashboard session, since it moves credentials. Pass `{"dataDir":"/path/to/.9router"}` for a
non-default location; otherwise `DATA_DIR` and `~/.9router` are searched.

- Reads both the current `db/data.sqlite` layout and the older flat `db.json`.
- The source database is opened **read-only** — a live 9Router install is never modified.
- **Additive and non-destructive.** Existing records are kept, duplicates are skipped by name, and
  the report lists every skip. Re-running is safe.
- **API keys are reported but not imported.** nullrouter stores only a digest, so a plaintext key
  cannot be turned into a usable record. Re-issue them from the dashboard.

The dashboard has a UI for this at `/dashboard/migrate` if you would rather not use curl.

## Configuration

Every service takes `NULLROUTER_<SERVICE>_HOST` and `NULLROUTER_<SERVICE>_PORT`, and most also accept
a bare `PORT` (which wins over the service-specific variable).

### State

| Variable | Default | Notes |
|---|---|---|
| `NULLROUTER_STATE_FILE` | *(unset)* | JSON state file. **Unset means in-memory only — nothing persists across restarts.** |
| `NULLROUTER_STATE_HOST` / `NULLROUTER_STATE_PORT` | `127.0.0.1:20134` | |

### Gateway

| Variable / flag | Default |
|---|---|
| `NULLROUTER_GATEWAY_LISTEN` / `--listen` | `127.0.0.1:20128` |
| `NULLROUTER_REQUIRE_API_KEY` / `--require-api-key` | `false` |
| `NULLROUTER_API_UPSTREAM` / `--api-upstream` | `127.0.0.1:20129` |
| `NULLROUTER_DASHBOARD_UPSTREAM` | `127.0.0.1:20130` |
| `NULLROUTER_CATALOG_UPSTREAM` | `127.0.0.1:20131` |
| `NULLROUTER_RUNTIME_UPSTREAM` | `127.0.0.1:20132` |
| `NULLROUTER_EVENTS_UPSTREAM` | `127.0.0.1:20133` |
| `NULLROUTER_STATE_UPSTREAM` | `127.0.0.1:20134` |
| `NULLROUTER_AUTH_UPSTREAM` | `127.0.0.1:20135` |

All upstreams must be loopback. `nullrouter-gateway --help` lists these too.

### Service-to-service

| Variable | Default | Used by |
|---|---|---|
| `NULLROUTER_STATE_ADDR` | `127.0.0.1:20134` | api, runtime, events |
| `NULLROUTER_RUNTIME_ADDR` | `127.0.0.1:20132` | api (dashboard chat forwarding) |

### Auth

| Variable | Default |
|---|---|
| `NULLROUTER_AUTH_SESSION_SECRET` | random 32 bytes per start (sessions die on restart) |
| `NULLROUTER_AUTH_PASSWORD_HASH` | *(unset)* — bcrypt hash; preferred |
| `INITIAL_PASSWORD` | `123456` — plaintext fallback |
| `AUTH_COOKIE_SECURE` | `false` |
| `NULLROUTER_AUTH_SESSION_TTL_SECONDS` | `86400` |
| `NULLROUTER_AUTH_LOCKOUT_THRESHOLD` | `5` |
| `NULLROUTER_AUTH_LOCKOUT_WINDOW_SECONDS` | `900` |
| `NULLROUTER_AUTH_LOCKOUT_DURATION_SECONDS` | `900` |
| `NULLROUTER_AUTH_LOCKOUT_CAPACITY` | `4096` |
| `NULLROUTER_AUTH_OIDC_TIMEOUT_SECONDS` | `10` |
| `NULLROUTER_AUTH_STATE_TIMEOUT_SECONDS` | `2` |
| `NULLROUTER_PUBLIC_ORIGIN` (or `BASE_URL`) | derived from the request | must match the redirect URI registered with your OIDC provider |

### Other

`NULLROUTER_DASHBOARD_STATIC` overrides the static asset root. `DATA_DIR` points 9Router discovery at
a non-default directory. `HEADROOM_URL` overrides the headroom proxy URL probed for status.
`RUST_LOG` controls tracing (the gateway defaults to `nullrouter_gateway=info`).

## What is deliberately not implemented

nullrouter would rather return an explicit `501` with `"unsupported": true` than a fabricated
success. Requests are still validated first, so you get a `400` for a malformed body rather than a
misleading `501`.

**Refused with an explicit 501:**

- **Bespoke provider executors** — the six protocols listed [above](#what-executes-and-what-refuses).
- **Headroom process control.** Detection is real: it finds a Python ≥ 3.10, asks pip which packages
  it holds, and reads the install log. Installing extras and starting/restarting the daemon are
  refused, because this service does not own that Python environment and has no supervisor for a
  detached daemon. A fake `{"success":true}` here would be the worst available lie — you would believe
  your prompts were being compressed while being billed for full-size requests.
- **Tunnel / Tailscale control.** Status reports honestly; every mutation is refused.
- **MITM control** (`/api/cli-tools/antigravity-mitm`) and its alias map. URL validation is real.
- **CLI tool config mutation** and Cowork MCP registry/tool discovery.
- **Provider OAuth *authorisation* flows** (`/api/oauth/*`) — device-code, PKCE browser flows, and
  vendor token imports for codex/cursor/kiro/gitlab/iflow. Getting a provider token for the first time
  has to happen outside this port. (Dashboard *sign-in* via OIDC is a separate subsystem and is fully
  implemented, and *refreshing* an existing provider token is implemented — see below.)
- **Proxy pool testing and relay deployment** to Cloudflare/Deno/Vercel. Pool CRUD is real; deploying
  a relay is not. Per-connection outbound proxies work.
- **Translator execution and log persistence.** Steps 1–3 validate and echo their shape; they do not
  translate. (The translation engine itself is real and used on the live `/v1` path — this is the
  dashboard's step-by-step inspector.)
- **Service lifecycle** (`/api/shutdown`, `/api/version/update`).
- **Database settings and proxy connectivity tests** under `/api/settings/*`.
- **SAML assertion consumption** — see [Security model](#security-model).

**Provider token refresh is real.** A token within its provider's refresh lead time is exchanged
before the call rather than after a 401, the rotation is persisted, and concurrent requests on one
expiring connection share a single exchange so a provider that invalidates a reused refresh token
cannot lock the account out. A rejected refresh token puts the connection into a re-auth cooldown
instead of retrying forever. Five providers (`kiro`, `github`, `vertex`, `vertex-partner`, `cursor`)
declare no refresh endpoint in upstream's own registry and are reported as needing manual replacement
rather than silently retried.

**Honest-but-empty surfaces:** MCP SSE at `/api/mcp/{plugin}/sse` connects and then reports
`backend_connected: false` with a reason; posting a message returns 503 rather than pretending to
deliver it. Console-log streams connect and emit an empty init frame — there is no live capture
backend.

**Model testing is real.** `POST /api/models/test` dispatches a one-token, non-streaming completion
through the runtime — the same path real traffic takes, so a passing test means something — and reports
latency, finish reason, and usage. Every failure carries **the provider's own message**: "insufficient
quota" or "The model `x` does not exist", not "request failed". A `200` that arrives without a
completion is reported as a failure, because some providers answer `200` with an error object and
calling that a working model is the one false pass this route exists to prevent. A model whose kind is
not `llm` is refused before dispatch rather than sent a chat body it would reject.

**Suggested-model lists are real**, and this is the one route deliberately *stricter* than upstream.
Eight providers publish a model catalogue; `GET /api/providers/suggested-models` fetches one and
filters it to the useful subset — for a gateway like OpenRouter, the free models with a context window
of 200k or more, largest first.

> **The `url` must be one the registry itself declares.** Upstream fetches whatever URL the caller
> passes. That is a server-side request forgery primitive: the route sits behind dashboard auth, but an
> authenticated request could still make the server GET any host it can reach — including this
> router's own internal services — and read the result back. The check is an exact URL match, not a
> host match, since a host match still allows every other path there. Nothing is lost, because the
> dashboard only ever passes a URL it read from the registry. The catalogue URLs are extracted from
> upstream by `tools/extract-models-fetcher.py` rather than typed in.

A catalogue that is down, slow, or malformed yields an empty list, matching upstream: this sits beside
a text field the user can always type into, so it must not present as a dashboard error. An *unknown
filter* is a 400 rather than an empty list, because an empty list would claim the provider publishes
no free models.

One divergence worth naming: upstream's filter table has no `openai` entry, yet four providers
(`perplexity-agent`, `venice`, `tokenrouter`, `vercel-ai-gateway`) declare `type: "openai"`, so
upstream answers 400 and their lists never populate. That filter is implemented here — a plain OpenAI
catalogue needs no filtering.

**Remote `/models` probing is implemented.** A user-added `openai-compatible-*` or
`anthropic-compatible-*` connection points at a host its owner chose, so the registry cannot know its
models: `/v1/models` asks the provider. A configured `providerSpecificData.enabledModels` still wins —
probing only fills the gap where there would otherwise be nothing to show. Results are cached for five
minutes, because editors poll this route on startup and sometimes per completion.

A failed probe leaves the configured list alone rather than emptying it: a provider that is briefly
slow should not take a working model picker away. Probes carry
`x-9r-internal-models-fetch: 1`, and a request arriving with that header is answered from
configuration without probing — a compatible node's base URL can point at another router, or at this
one, and without the guard the two would probe each other on every call. Upstream sets and honours the
same header, so the guard holds in a mixed deployment.

**PXPIPE (the Token Saver page, 8 routes)** is implemented, and is the one feature here that cannot
be pure Rust. It renders bulky Claude-format context into dense PNGs, which bill by pixel rather than
by token; the compression itself is the `pxpipe-proxy` npm package, and reimplementing PNG-packed
context rendering would be a different program with different output, not a port. So `crates/pxpipe`
installs that package and drives it through a long-lived `node` worker over a line-delimited pipe —
one worker per router rather than one per request, because Node's start-up would otherwise eat the
saving. It fails open at every step: no Node, no package, a timeout, a malformed reply, a Node below
the package's `>=20.19` requirement — each dispatches the original request unchanged and records why.
The worker lives in `nullrouter-runtime`, since that is where the transform runs; `nullrouter-api`
proxies the control routes to it and reads the event log directly. Upstream ships this surface built
but unreachable (its toggle sits behind a `{false && …}`, its sidebar entry commented out, marked
"experimental"); it is reachable here, and off by default.

`/v1/videos/*` is implemented: `generations`, `edits` and `extensions` create a job, `GET /v1/videos/{id}`
polls it, and a poll is pinned to the account that created the job — a different account cannot see
another's job id. Only 401/403/429 rotate to another account; a provider-side job failure is that
job's answer and is not retried elsewhere.

**Storage:** state is a JSON file with a bounded 1000-request ring, not SQLite. A 9Router SQLite
install imports into it.

## Development

```bash
cargo test --workspace          # 1117 tests, 132 integration files
cargo clippy --workspace --all-targets
cargo fmt --all --check
```

Tests by area:

| Area | Integration files | Tests |
|---|---|---|
| `apps/dashboard-leptos` | 26 | 302 |
| `crates/translate` | 4 | 149 |
| `services/api-actix` | 24 | 133 |
| `crates/execute` | 5 | 102 |
| `services/runtime-actix` | 18 | 95 |
| `services/state-actix` | 14 | 78 |
| `crates/pxpipe` | 2 | 68 |
| `services/gateway-pingora` | 12 | 57 |
| `crates/providers` | — | 43 |
| `services/dashboard-actix` | 10 | 34 |
| `services/events-actix` | 7 | 20 |
| `crates/contracts` | 2 | 13 |
| `services/catalog-actix` | 6 | 12 |
| `services/auth-actix` | 2 | 11 |

Two suites need a real `node` on the `PATH` and **fail rather than skip** without one, because the
PXPIPE transform is a JavaScript library and a suite that passed quietly would report the feature as
covered when nothing had run: `crates/pxpipe/tests/worker.rs` drives a real worker process, and
`services/runtime-actix/tests/pxpipe_request_path.rs` asserts that the transformed body is what the
provider actually receives. A third, `crates/pxpipe/tests/real_package.rs`, installs `pxpipe-proxy`
from npm and is off unless asked for:

```bash
PXPIPE_TEST_INSTALL=1 cargo test -p nullrouter-pxpipe --test real_package
```

It earns its keep: it is what caught a reason mapping written against a shape the package does not
emit, which every stub test had passed.

Beyond unit and contract coverage, the suite includes **boundary tests** (what each service must
refuse), **characterization tests** (upstream behaviour pinned so a port cannot drift), and **regression
tests** named after the defect they lock down.

### Lints

The workspace runs deliberately strict, workspace-wide:

```toml
clippy::all         = "deny"     # not warn
clippy::pedantic    = "warn"
clippy::nursery     = "warn"
unwrap_used         = "deny"
expect_used         = "deny"
panic               = "deny"
todo                = "deny"
unimplemented       = "deny"
unreachable         = "deny"
indexing_slicing    = "deny"
undocumented_unsafe_blocks = "deny"
```

A panic in a request path is a lint error, not a code review note. Release builds use fat LTO, one
codegen unit, stripped symbols, and `panic = "abort"`.

### Parity reference

Registry and capability tables are generated from a read-only checkout of upstream 9Router, which is
**not vendored** in this repository (it carries its own `.git` and is gitignored at `/inspire`). The
generated JSON under `crates/providers/data/` is committed, so nothing here requires that checkout to
build, test, or run. Clone it only if you are working on parity:

```bash
git clone https://github.com/decolua/9router inspire
```

## License

MIT, per the workspace manifest. Note that no `LICENSE` file is currently committed at the repository
root.

---

Version `0.5.20`. Registry generated from upstream `v0.5.55`.
