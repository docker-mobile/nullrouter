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
#![allow(clippy::future_not_send, reason = "wasm32 is single-threaded; JsFuture is !Send")]

pub mod api;
pub mod i18n;
pub mod routes;
pub mod shell;
pub mod theme;

/// Mount the dashboard into the document body.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(App);
}

/// Native builds have no document to mount into.
#[cfg(not(target_arch = "wasm32"))]
pub const fn main() {}

/// The application root: providers, then the router.
///
/// Theme is installed before anything renders so the first painted frame is already in the right
/// scheme. The locale load is awaited in a `Suspense` rather than blocking the mount, so a slow or
/// missing locale file degrades to a brief loading state instead of a blank page.
#[cfg(target_arch = "wasm32")]
#[leptos::component]
pub fn App() -> impl leptos::IntoView {
    use leptos::prelude::*;
    use leptos_router::components::{Route, Router, Routes};
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
                            <shell::Shell>
                                <Routes fallback=routes::NotFound>
                                    <Route path=path!("/dashboard") view=routes::Overview />
                                    <Route path=path!("/dashboard/routing") view=routes::Routing />
                                    <Route path=path!("/dashboard/keys") view=routes::Keys />
                                    <Route path=path!("/dashboard/usage") view=routes::Usage />
                                    <Route path=path!("/dashboard/logs") view=routes::Logs />
                                    <Route path=path!("/dashboard/settings") view=routes::Settings />
                                </Routes>
                            </shell::Shell>
                        </Router>
                    }
                })
            }}
        </Suspense>
    }
}
