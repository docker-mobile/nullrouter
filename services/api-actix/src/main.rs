use actix_web::{App, HttpServer, web};
use nullrouter_api::{AppConfig, RuntimeClient, StateClient, configure};

const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 20129;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let server = ServerConfig::from_env();
    let app_config = AppConfig::default();
    // Holds a connection pool, so it is built once and shared across workers.
    let state = web::Data::new(StateClient::from_env());
    let runtime = web::Data::new(RuntimeClient::from_env());
    // Reads the install directory and the event log. Shared rather than per-worker
    // so every worker reports the same install state.
    let token_saver = web::Data::new(nullrouter_pxpipe::TokenSaver::discover());

    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(app_config.clone()))
            // Without this the usage endpoints would fail at runtime, since
            // they extract `web::Data<StateClient>`.
            .app_data(state.clone())
            .app_data(runtime.clone())
            .app_data(token_saver.clone())
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
            host: std::env::var("NULLROUTER_API_HOST").unwrap_or_else(|_| DEFAULT_HOST.to_owned()),
            port: std::env::var("PORT")
                .or_else(|_| std::env::var("NULLROUTER_API_PORT"))
                .ok()
                .and_then(|value| value.parse::<u16>().ok())
                .unwrap_or(DEFAULT_PORT),
        }
    }
}
