#![allow(clippy::option_if_let_else)]
#![allow(clippy::format_push_string)]
mod console_logs;
mod mcp;
mod routes;
mod usage_stream;

pub use mcp::McpBridge;
pub use routes::configure;

pub const SERVICE_NAME: &str = "nullrouter-events";
pub const DEFAULT_HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 20133;

pub use usage_stream::UsageReader;
