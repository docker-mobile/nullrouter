# nullrouter-dashboard-wasm

Leptos CSR/WASM dashboard app for the Nullrouter 9Router port.

Check and build from the repo root:

```bash
cargo test -p nullrouter-dashboard-wasm --test dashboard_data
cargo build -p nullrouter-dashboard-wasm --lib --target wasm32-unknown-unknown
wasm-bindgen target/wasm32-unknown-unknown/debug/nullrouter_dashboard_wasm.wasm \
  --target web --out-dir apps/dashboard-leptos/dist --out-name dashboard_leptos
```

The Actix dashboard host imports `/pkg/dashboard_leptos.js` and `/pkg/dashboard_leptos_bg.wasm`.
Copy `apps/dashboard-leptos/dist/*` to the host `/pkg` static directory. The authoritative dashboard
stylesheet is owned by `services/dashboard-actix/static/assets/dashboard.css`.
