//! The dashboard, compiled to WebAssembly and served by `nullrouter-dashboard-host`.
//!
//! Everything that can be decided without a browser lives in plain Rust so it stays testable on
//! the native target: routing tables, request shaping, locale resolution, theme resolution. Only
//! the calls that need a `Window` sit behind `#[cfg(target_arch = "wasm32")]`, and each has a
//! native counterpart that reports the absence rather than faking a result.

// Every async fn that touches the DOM awaits a `JsFuture`, which holds an `Rc` and is therefore
// `!Send`. That is a property of the platform -- wasm32 is single-threaded and there is no other
// thread to send a future to -- not something callers here could fix, so the nursery lint asking
// for `Send` futures cannot be satisfied in a browser crate.
#![allow(
    clippy::future_not_send,
    reason = "wasm32 is single-threaded; JsFuture is !Send"
)]

pub mod api;
pub mod i18n;
pub mod routes;
pub mod shell;
pub mod theme;

/// Mount the dashboard into the document body.
///
/// Named `start` rather than `main` so `cargo test --target wasm32-unknown-unknown` can
/// compile: the test harness already emits a `main`, and a second one fails with
/// "entry symbol `main` declared multiple times".
#[cfg(all(target_arch = "wasm32", not(test)))]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}

/// The application root: providers, then the router.
///
/// Theme is installed before anything renders so the first painted frame is already in the right
/// scheme.
///
/// Nothing here is gated on a network fetch. The router used to be wrapped in a `Suspense` waiting
/// on the locale file, which meant one unresolved request left every page showing a spinner
/// indefinitely — no error, no content, no way for the user to tell it apart from a slow load. The
/// English catalogue is compiled in ([`i18n::Locale::embedded`]), so a complete locale exists before
/// the first frame and a different language is loaded over the top of it afterwards.
#[cfg(target_arch = "wasm32")]
#[leptos::component]
pub fn App() -> impl leptos::IntoView {
    use leptos::prelude::*;
    use leptos_router::components::{ParentRoute, Route, Router, Routes};
    use leptos_router::path;

    theme::provide_theme();

    // Synchronous: the embedded catalogue needs no await, so the router below renders on the first
    // frame with real labels.
    i18n::provide_embedded_locale();

    // A non-English preference is loaded after mount and replaces the context when it arrives.
    // Failure is not surfaced as an error state on purpose: English is already on screen, and a
    // missing translation file is not something the user can act on.
    i18n::spawn_preferred_locale();

    view! {
        <Router>
            <Routes fallback=routes::NotFound>
                <Route path=path!("/login") view=routes::Login />
                <Route path=path!("/callback") view=routes::Callback />
                <ParentRoute path=path!("/dashboard") view=shell::DashboardFrame>
                    <Route path=path!("") view=routes::Overview />
                    <Route path=path!("providers") view=routes::Providers />
                    <Route path=path!("models") view=routes::Models />
                    <Route path=path!("combos") view=routes::Combos />
                    <Route path=path!("pricing") view=routes::Pricing />
                    <Route path=path!("keys") view=routes::Keys />
                    <Route path=path!("usage") view=routes::Usage />
                    <Route path=path!("logs") view=routes::Logs />
                    <Route path=path!("cli-tools") view=routes::CliTools />
                    <Route path=path!("pxpipe") view=routes::Pxpipe />
                    <Route path=path!("headroom") view=routes::Headroom />
                    <Route path=path!("tunnel") view=routes::tunnel::Tunnel />
                    <Route path=path!("proxy-pools") view=routes::proxy_pools::ProxyPools />
                    <Route path=path!("nodes") view=routes::provider_nodes::ProviderNodes />
                    <Route path=path!("import") view=routes::oauth_import::OauthImport />
                    <Route path=path!("translator") view=routes::translator::Translator />
                    <Route path=path!("catalog") view=routes::catalog::Catalog />
                    <Route path=path!("settings") view=routes::Settings />
                    <Route path=path!("*rest") view=routes::StatusPage />
                </ParentRoute>
            </Routes>
        </Router>
    }
}
