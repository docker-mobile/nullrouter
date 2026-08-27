use actix_web::{App, HttpServer, web};
use nullrouter_events::{DEFAULT_HOST, DEFAULT_PORT, UsageReader, configure};

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let server = ServerConfig::from_env();
    // Holds a connection pool, so it is built once and shared across workers.
    let usage = web::Data::new(UsageReader::from_env());

    HttpServer::new(move || {
        App::new()
            // The live usage stream extracts this; without it the route would
            // fail at runtime.
            .app_data(usage.clone())
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
            host: std::env::var("NULLROUTER_EVENTS_HOST")
                .unwrap_or_else(|_| DEFAULT_HOST.to_owned()),
            port: std::env::var("PORT")
                .or_else(|_| std::env::var("NULLROUTER_EVENTS_PORT"))
                .ok()
                .and_then(|value| value.parse::<u16>().ok())
                .unwrap_or(DEFAULT_PORT),
        }
    }
}
