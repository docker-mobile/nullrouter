pub mod account;
pub mod api;
pub mod dashboard;
pub mod i18n;
pub mod ui;

pub use ui::App;

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn main() {
    console_error_panic_hook::set_once();
    leptos::mount::mount_to_body(|| leptos::prelude::view! { <App /> });
}

#[cfg(not(target_arch = "wasm32"))]
pub const fn main() {}
