mod api_keys;
mod console_logs;
mod internal;
mod migrate;
mod provider_nodes;
mod responses;
mod routes;
mod store;
mod usage;

pub use store::{FLUSH_INTERVAL, ProviderConnection, StateStore, StoreError};

pub const SERVICE_NAME: &str = "nullrouter-state";
pub const DEFAULT_HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 20134;

pub fn configure(config: &mut actix_web::web::ServiceConfig) {
    api_keys::configure(config);
    routes::configure(config);
    provider_nodes::configure(config);
    internal::configure(config);
    console_logs::configure(config);
}
