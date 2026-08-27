use actix_web::{App, HttpServer, web};
use nullrouter_runtime::{DEFAULT_HOST, DEFAULT_PORT, Runtime, app_config, configure};

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let server = ServerConfig::from_env();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // The executor and state client hold connection pools, so they are built
    // once and shared across workers rather than per-worker.
    let runtime = web::Data::new(Runtime::new());

    HttpServer::new(move || {
        App::new()
            // Previously only registered by the tests, which left `/health`
            // returning 500 in the real binary.
            .app_data(web::Data::new(app_config()))
            .app_data(runtime.clone())
            .configure(configure)
    })
    .bind((server.host.as_str(), server.port))?
    .run()
    .await
}

#[derive(Debug)]
struct ServerConfig {
    host: String,
    port: u16,
}

impl ServerConfig {
    fn from_env() -> Self {
        Self {
            host: std::env::var("NULLROUTER_RUNTIME_HOST")
                .unwrap_or_else(|_| DEFAULT_HOST.to_owned()),
            port: std::env::var("PORT")
                .or_else(|_| std::env::var("NULLROUTER_RUNTIME_PORT"))
                .ok()
                .and_then(|value| value.parse::<u16>().ok())
                .unwrap_or(DEFAULT_PORT),
        }
    }
}
