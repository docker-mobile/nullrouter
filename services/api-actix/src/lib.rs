mod catalog;
mod cli_tools;
mod combos;
mod errors;
mod handlers;
mod headroom;
mod json_body;
mod keys;
mod lifecycle;
mod locale;
mod media_providers;
mod migrate;
mod model_tools;
mod models;
mod oauth;
mod provider_nodes;
mod provider_probe;
mod provider_tools;
mod providers;
mod proxy_pool_tools;
mod relay_deploy;
mod proxy_pools;
mod proxy_test;
mod pxpipe;
mod responses;
mod routes;
mod settings_defaults;
mod state_client;
mod translator;
mod tunnel;
mod usage;

pub use lifecycle::ShutdownHandle;
pub use routes::configure;
pub use state_client::{RuntimeClient, StateClient};
pub use tunnel::Manager as TunnelManager;

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub version: &'static str,
}

impl AppConfig {
    #[must_use]
    pub const fn new(version: &'static str) -> Self {
        Self { version }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self::new(env!("CARGO_PKG_VERSION"))
    }
}
