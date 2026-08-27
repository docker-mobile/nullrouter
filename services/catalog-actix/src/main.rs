use actix_web::{App, HttpServer};
use nullrouter_catalog::{DEFAULT_HOST, DEFAULT_PORT, configure};

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let server = ServerConfig::from_env();

    HttpServer::new(|| App::new().configure(configure))
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
            host: std::env::var("NULLROUTER_CATALOG_HOST")
                .unwrap_or_else(|_| DEFAULT_HOST.to_owned()),
            port: std::env::var("PORT")
                .or_else(|_| std::env::var("NULLROUTER_CATALOG_PORT"))
                .ok()
                .and_then(|value| value.parse::<u16>().ok())
                .unwrap_or(DEFAULT_PORT),
        }
    }
}
