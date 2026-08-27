# Dashboard Actix Host

Actix Web host for the dashboard frontend. This service is intentionally limited to static dashboard hosting and health checks; API routes stay in the API service and gateway routing stays outside this crate.

Expected built Leptos assets from the `nullrouter-dashboard-wasm` package under `apps/dashboard-leptos`:

- `apps/dashboard-leptos/dist/dashboard_leptos.js` -> `services/dashboard-actix/static/pkg/dashboard_leptos.js`
- `apps/dashboard-leptos/dist/dashboard_leptos_bg.wasm` -> `services/dashboard-actix/static/pkg/dashboard_leptos_bg.wasm`
- Additional wasm-bindgen snippets/assets emitted under `apps/dashboard-leptos/dist/` -> `services/dashboard-actix/static/pkg/`

The Leptos app currently uses Trunk with `public_url = "/dashboard/"`. Pingora can still mount this
host at the dashboard routes while this service exposes the copied WASM package under `/pkg/*`.

Provider/static assets:

- Provider images are served from `services/dashboard-actix/static/providers/*` at `/providers/*`.
- Shared static files are served from `services/dashboard-actix/static/assets/*` at `/assets/*`.
