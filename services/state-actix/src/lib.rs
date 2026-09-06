mod api_keys;
mod at_rest;
mod console_logs;
mod internal;
mod migrate;
mod provider_nodes;
mod responses;
mod routes;
mod store;
mod usage;

// `AtRestError` is re-exported because `StoreError` wraps it: without this a caller outside the crate
// could receive the variant but not name the type inside it.
pub use at_rest::AtRestError;
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
