mod catalog;
mod provider_inventory;
mod route_inventory;
mod routes;
mod state;

pub use routes::configure;

pub const DEFAULT_HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 20131;
