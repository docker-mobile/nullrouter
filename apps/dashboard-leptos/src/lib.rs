//! The dashboard, compiled to WebAssembly and served by `nullrouter-dashboard-host`.
//!
//! Everything that can be decided without a browser lives in plain Rust so it stays testable on
//! the native target: routing tables, request shaping, locale resolution, theme resolution. Only
//! the calls that need a `Window` sit behind `#[cfg(target_arch = "wasm32")]`, and each has a
//! native counterpart that reports the absence rather than faking a result.

pub mod theme;

/// Mount the dashboard into the document body.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(|| leptos::prelude::view! { <Root /> });
}

/// Native builds have no document to mount into.
#[cfg(not(target_arch = "wasm32"))]
pub const fn main() {}

/// The application root.
#[cfg(target_arch = "wasm32")]
#[leptos::component]
fn Root() -> impl leptos::IntoView {
    use leptos::prelude::*;

    let theme = theme::provide_theme();

    view! {
        <main class="min-h-dvh bg-background text-foreground grid place-items-center">
            <div class="space-y-4 text-center">
                <h1 class="text-2xl font-semibold tracking-tight">"nullrouter"</h1>
                <p class="text-sm text-muted-foreground">
                    "Theme resolves to "
                    <code class="font-mono">{move || theme.resolved.get().as_str()}</code>
                </p>
            </div>
        </main>
    }
}
