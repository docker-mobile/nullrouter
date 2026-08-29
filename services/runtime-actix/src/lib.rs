mod combo;
mod errors;
mod fusion;
mod handlers;
mod models;
mod pipeline;
mod pxpipe;
mod requests;
mod responses;
mod routes;
mod state_client;
mod video;

pub use pipeline::Runtime;
pub use routes::configure;

pub const SERVICE_NAME: &str = "nullrouter-runtime";
pub const DEFAULT_HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 20132;

#[derive(Debug, Clone, Copy)]
pub struct AppConfig {
    pub service_name: &'static str,
}

pub const fn app_config() -> AppConfig {
    AppConfig {
        service_name: SERVICE_NAME,
    }
}
