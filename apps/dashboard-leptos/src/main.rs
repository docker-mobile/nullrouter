#[cfg(target_arch = "wasm32")]
fn main() {
    nullrouter_dashboard_wasm::main();
}

#[cfg(not(target_arch = "wasm32"))]
const fn main() {
    nullrouter_dashboard_wasm::main();
}
