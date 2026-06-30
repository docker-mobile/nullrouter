//! nullrouter-dashboard — Leptos SSR + WASM hydrate dashboard.
//!
//! Ports `inspire/src/app/(dashboard)/` plus sibling pages (landing, login).
//! Builds as both an `rlib` (linked into `nullrouter-server` for SSR via
//! `leptos_actix`) and a `cdylib` (WASM target for client hydration).

#![forbid(unsafe_code)]
