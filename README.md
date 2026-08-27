# Nullrouter

Nullrouter is a Rust microservice port of 9Router. The upstream reference checkout lives in
`inspire/` and is intentionally read-only; the Rust implementation is split into separate services
behind a single public port, with shared contracts.

## Services

All services except the gateway bind to loopback only.

| Service | Binary | Address | Role |
|---|---|---|---|
| `services/gateway-pingora` | `nullrouter-gateway` | `127.0.0.1:20128` | The one public port; routing, auth policy |
| `services/api-actix` | `nullrouter-api` | `127.0.0.1:20129` | Dashboard/bootstrap APIs, usage aggregates |
| `services/dashboard-actix` | `nullrouter-dashboard-host` | `127.0.0.1:20130` | Static/WASM host |
| `services/catalog-actix` | `nullrouter-catalog` | `127.0.0.1:20131` | Catalog/route inventory |
| `services/runtime-actix` | `nullrouter-runtime` | `127.0.0.1:20132` | **Provider execution** (`/v1`, `/v1beta`) |
| `services/events-actix` | `nullrouter-events` | `127.0.0.1:20133` | SSE streams (usage, console logs, MCP) |
| `services/state-actix` | `nullrouter-state` | `127.0.0.1:20134` | Persistence, credentials, usage records |
| `services/auth-actix` | `nullrouter-auth` | `127.0.0.1:20135` | Sessions and authorization |
| `apps/dashboard-leptos` | `nullrouter-dashboard-wasm` | — | Leptos CSR dashboard |

Shared crates:

- `crates/contracts` — typed response contracts.
- `crates/providers` — provider registry (117 providers, 850 models), per-model capabilities, model
  resolution, format detection.
- `crates/translate` — request/response translation across OpenAI Chat, OpenAI Responses, Claude,
  and Gemini formats.
- `crates/execute` — provider HTTP execution: auth, retry, URL fallback, SSE streaming.

Registry data is generated from the `inspire/` reference (currently upstream `v0.5.55`) rather than
hand-transcribed, so transports, auth descriptors, model tables, and per-model limits stay faithful.

## Provider execution

Requests to `/v1/chat/completions`, `/v1/responses`, `/v1/messages`, `/v1/api/chat`, and native
Gemini `/v1beta/models/{model}:generateContent` are executed against real providers:

- **Format translation** in both directions between `openai`, `openai-responses`, `claude`, and
  `gemini`, pivoting through OpenAI as upstream does. A Claude client can drive an OpenAI-format
  provider and receive native Claude SSE events, and vice versa.
- **The OpenAI Responses API** (`/v1/responses`) speaks its real protocol: `input[]`/`instructions`
  are regrouped into chat turns on the way out, and the reply is a sequence of named lifecycle
  events (`response.created`, `response.output_text.delta`, `response.completed`, …) with monotonic
  `sequence_number` and every opened item explicitly closed — not reshaped chat chunks.
- **Incremental streaming**: frames reach the client as each upstream chunk is parsed, so
  time-to-first-token tracks the provider's own latency rather than the full completion time. Memory
  is bounded by a small channel, and a slow client applies backpressure instead of losing frames.
- **Per-model output ceilings** from the registry's capability table, so a 128k-output model is not
  clamped to the conservative 64000 default and truncated.
- **Streaming and non-streaming**, including collapsing a forced-stream provider back into a single
  JSON body when the client asked for JSON.
- **Non-chat services** — `/v1/embeddings`, `/v1/images/generations`, `/v1/audio/speech`,
  `/v1/audio/transcriptions`, `/v1/search`, `/v1/web/fetch` — dispatch to each provider's
  service-specific endpoint from the registry.
- **Account fallback**: on a retryable failure the next account is tried, with per-model cooldowns
  and exponential backoff for quota errors.
- **`/v1/models`** is registry-backed and scoped to configured connections.
- **Usage** is recorded per request and exposed through `/api/usage/*`, with a live
  `/api/usage/stream` SSE feed.

The provider registry and per-service endpoint tables are generated from the frozen `inspire/`
reference rather than hand-transcribed, so transports, auth descriptors, and model tables stay
faithful to upstream.

### Supported provider protocols

The OpenAI-compatible, Anthropic-compatible, and Gemini protocol families execute — the large
majority of the registry, including `openai`, `anthropic`, `gemini`, `groq`, `deepseek`,
`openrouter`, `mistral`, `cerebras`, `together`, `xai`, and the dynamic `openai-compatible-*` /
`anthropic-compatible-*` families.

Providers whose wire protocol needs a bespoke executor return an explicit `501` naming the provider:
`kiro`, `cursor`, `codex`, `antigravity`, `gemini-cli`, `commandcode`, `grok-web`,
`perplexity-web`, `ollama`.

## Migrating from 9Router

An existing 9Router installation can be imported in place. Provider connections (including API keys
and OAuth tokens), combos, proxy pools, and routing settings carry over.

```bash
# Preview what would be imported, without writing anything.
curl -X POST http://127.0.0.1:20128/api/migrate/9router \
  -H 'content-type: application/json' -d '{"dryRun":true}'

# Import.
curl -X POST http://127.0.0.1:20128/api/migrate/9router
```

The endpoint requires a dashboard session, since it imports credentials. Pass
`{"dataDir":"/path/to/.9router"}` to point at a non-default location; otherwise `DATA_DIR` and
`~/.9router` are searched.

- Reads both the current `db/data.sqlite` layout and the older flat `db.json`.
- The source database is opened **read-only**, so a live 9Router install is never modified.
- Additive and non-destructive: existing records are kept, duplicates are skipped by name, and the
  report lists every skip. Re-running is safe.
- **API keys are reported but not imported**: nullrouter stores only a digest of each key, so an
  existing plaintext key cannot be turned into a usable record. Re-issue them from the dashboard.

## Security boundaries

- Only the gateway listens publicly; every other service binds loopback.
- `/internal/*` is refused at the gateway from every peer and route. Those endpoints return
  **unredacted credentials** to the runtime and are safe only because of that refusal
  (`services/gateway-pingora/tests/internal_boundary.rs` pins it).
- Stored secrets (`apiKey`, `accessToken`, `refreshToken`) are stripped from every public API response.
- `requireApiKey` is enforced in the runtime before any provider call, so the persisted setting
  actually takes effect.

## Run locally

Build the dashboard WASM once:

```bash
cargo build -p nullrouter-dashboard-wasm --lib --target wasm32-unknown-unknown --release
wasm-bindgen --target web \
  --out-dir services/dashboard-actix/static/pkg \
  --out-name dashboard_leptos \
  target/wasm32-unknown-unknown/release/nullrouter_dashboard_wasm.wasm
```

Then start the services. `nullrouter-state` should come up first, since the runtime and API read
credentials and usage from it:

```bash
cargo run -p nullrouter-state
cargo run -p nullrouter-runtime
cargo run -p nullrouter-api
cargo run -p nullrouter-events
cargo run -p nullrouter-catalog
cargo run -p nullrouter-auth
cargo run -p nullrouter-dashboard-host
cargo run -p nullrouter-gateway --bin nullrouter-gateway
```

Open `http://127.0.0.1:20128/dashboard/endpoint`.

Add a provider connection, then call through the single public port:

```bash
curl -X POST http://127.0.0.1:20128/v1/chat/completions \
  -H 'content-type: application/json' \
  -d '{"model":"openai/gpt-5","messages":[{"role":"user","content":"hello"}]}'
```

### Configuration

- `NULLROUTER_STATE_ADDR`, `NULLROUTER_RUNTIME_ADDR` — loopback addresses services use to reach each
  other.
- `NULLROUTER_STATE_PATH` — JSON state file location.
- `NULLROUTER_<SERVICE>_HOST` / `NULLROUTER_<SERVICE>_PORT`, or `PORT`, per service.

## Not ported

- **Full OAuth authorization flows** (device-code, PKCE browser flows, vendor token imports for
  codex/cursor/kiro/gitlab/iflow). Generic refresh-token grants are modeled; initial authorization
  is not.
- **Bespoke provider executors** for the protocols listed above.
- **Remote `/models` probing** for dynamic compatible providers: set
  `providerSpecificData.enabledModels` to list their models.
- MITM proxy, headroom compression, RTK/caveman/ponytail token savers, tunnel/tailscale, MCP stdio
  bridging, and combo rotation/fusion strategies (a combo resolves to its first model).
- Provider-native thinking normalization: the registry's `thinkingFormat` is carried in the
  capability table but not yet applied to outbound requests.
- `pxpipe` (8 routes) and `/v1/videos/*`: **deliberately excluded**, not deferred. `pxpipe` is an
  external binary subsystem that upstream itself keeps commented out of its sidebar.
