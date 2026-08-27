# Nullrouter Design System

This port follows the local 9Router reference under `inspire/`, tracked at upstream
`699edac3273e13d4744bc46f6082618f08560702` (v0.5.55). Registry, per-service endpoint, and
per-model capability tables are generated from that checkout rather than hand-transcribed.

## 1. Product Surface

Primary surfaces for the first Rust slice:

- Dashboard-compatible entry at `/`, `/landing`, `/dashboard`, and `/dashboard/endpoint`.
- Active in-shell sections for Endpoint, Providers, Usage, Status, and Settings.
- OpenAI-compatible local API under `/v1`.

The reference root redirects to `/dashboard`, so the Rust first screen uses the dashboard endpoint
surface rather than the promotional landing page. Non-ported dashboard areas are rendered inactive
instead of being advertised as working features.

## 2. Tokens

Reference values come from `inspire/src/app/globals.css`,
`inspire/src/shared/components/layouts/DashboardLayout.js`, `Sidebar.js`, and `Header.js`.

- `brand`: `#E56A4A` for active dashboard controls.
- `brand-hover`: `#cc5236`.
- `bg-dark`: `#1a1a1a`.
- `surface-dark`: `#262626`.
- `surface-dark-2`: `#303030`.
- `border-dark`: `#333333`.
- `border-subtle-dark`: `#2a2a2a`.
- `text-main-dark`: `#ededed`.
- `text-muted-dark`: `#9ca3af`.
- `radius-sm`: `8px`.
- `radius-md`: `10px`.
- `radius-lg`: `14px`.
- `shadow-warm`: `0 2px 12px -2px rgba(229, 106, 74, 0.25)`.
- `shadow-soft`: `0 1px 2px rgba(0, 0, 0, 0.3)`.

## 3. Typography

Use a browser-safe sans stack in the Rust shell so headless Chrome does not choose a bitmap-local
Inter variant:

`Arial, "Helvetica Neue", Helvetica, -apple-system, BlinkMacSystemFont, "Segoe UI", system-ui, sans-serif`.

Dashboard and API copy uses smaller dense labels with clear muted secondary text. There is no
hero-scale typography in the first viewport.

## 4. Layout

- Desktop uses the reference dashboard shell: fixed left sidebar, traffic lights, compact logo, dense nav, and a top header.
- The main pane uses a warm dark base, faint brand grid, and dashboard cards.
- The first content area is the Endpoint page with local endpoint rows and API key state.
- Usage uses the upstream topology/recent-requests structure, but displays zeroed telemetry until the
  Rust host exposes usage APIs or an event stream.
- Mobile keeps navigation functional as a horizontally scrollable dashboard rail.

## 5. Primitives

- `nr-sidebar-item`: 34px dense row, 8px radius, active brand tint.
- `nr-button-primary`: brand fill, white text, 10px radius, warm shadow.
- `nr-button-secondary`: dashboard surface fill, border, muted text.
- `nr-card`: `surface-dark`, `border-subtle-dark`, 14px radius.
- `nr-endpoint-row`: fixed label chip, monospace endpoint field, compact action square.
- `nr-status-alert`: amber diagnostic card for currently stubbed execution.
- `nr-provider-tile`: provider logo, dashboard surface, border, compact label.
- `nr-usage-topology`: 9Router hub plus provider nodes on a muted dashboard canvas; no active flow
  animation until live requests are wired.
- `nr-usage-log`: compact recent-request list with an empty telemetry state.

## 6. Motion

Motion is intentionally quiet for this static Rust shell. `prefers-reduced-motion` still disables
any future continuous animation.

## 7. Current Accepted Debt

Provider execution and usage telemetry are implemented: requests reach real providers with
bidirectional format translation, and usage metrics come from recorded requests with a live
`/api/usage/stream` SSE feed.

Remaining debt:

- Usage is still an in-shell `#usage` route rather than a host-level `/dashboard/usage` route, and
  topology nodes are catalog previews rather than live provider connections.
- Full OAuth authorization flows (device-code, PKCE, vendor token imports) are not ported; only
  generic refresh-token grants are modeled.
- Providers needing bespoke executors (`kiro`, `cursor`, `codex`, `antigravity`, `gemini-cli`,
  `commandcode`, `grok-web`, `perplexity-web`, `ollama`) return an explicit 501 naming the provider
  rather than a wrong answer.
- Provider-native thinking normalization is not applied: the registry's `thinkingFormat` is carried
  in the capability table but not yet used to shape outbound requests.
- State persists to JSON with a bounded ring of recent requests, not SQLite. A 9Router SQLite
  install can be imported into it (see README), but API keys cannot carry over because only digests
  are stored.
- MITM, headroom compression, token savers, tunnel/tailscale, MCP stdio bridging, and combo
  rotation/fusion remain future porting work.
