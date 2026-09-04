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
/// scheme. The locale load is awaited in a `Suspense` rather than blocking the mount, so a slow or
/// missing locale file degrades to a brief loading state instead of a blank page.
#[cfg(target_arch = "wasm32")]
#[leptos::component]
pub fn App() -> impl leptos::IntoView {
    use leptos::prelude::*;
    use leptos_router::components::{ParentRoute, Route, Router, Routes};
    use leptos_router::path;

    theme::provide_theme();

    let locale = LocalResource::new(i18n::provide_locale);

    view! {
        <Suspense fallback=|| {
            view! {
                <div class="min-h-dvh bg-background grid place-items-center">
                    <div class="size-6 rounded-full border-2 border-muted border-t-foreground animate-spin" />
                </div>
            }
        }>
            // Read the resource so Suspense actually waits on it; the locale itself reaches
            // components through context rather than as a prop.
            {move || {
                locale.get().map(|_| {
                    view! {
                        <Router>
                            <Routes fallback=routes::NotFound>
                                <Route path=path!("/login") view=routes::Login />
                                <Route path=path!("/callback") view=routes::Callback />
                                <ParentRoute path=path!("/dashboard") view=shell::DashboardFrame>
                                    <Route path=path!("") view=routes::Overview />
                                    <Route path=path!("providers") view=routes::Providers />
                                    <Route path=path!("keys") view=routes::Keys />
                                    <Route path=path!("usage") view=routes::Usage />
                                    <Route path=path!("logs") view=routes::Logs />
                                    <Route path=path!("settings") view=routes::Settings />
                                    <Route path=path!("*rest") view=routes::StatusPage />
                                </ParentRoute>
                            </Routes>
                        </Router>
                    }
                })
            }}
        </Suspense>
    }
}
