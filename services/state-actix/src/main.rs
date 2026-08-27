use std::path::PathBuf;

use actix_web::{App, HttpServer, web};
use nullrouter_state::{DEFAULT_HOST, DEFAULT_PORT, StateStore, configure};

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let server = ServerConfig::from_env();
    let store = build_store(server.state_file.as_ref())?;

    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(store.clone()))
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
    state_file: Option<PathBuf>,
}

impl ServerConfig {
    fn from_env() -> Self {
        Self {
            host: std::env::var("NULLROUTER_STATE_HOST")
                .unwrap_or_else(|_| DEFAULT_HOST.to_owned()),
            port: std::env::var("PORT")
                .or_else(|_| std::env::var("NULLROUTER_STATE_PORT"))
                .ok()
                .and_then(|value| value.parse::<u16>().ok())
                .unwrap_or(DEFAULT_PORT),
            state_file: std::env::var_os("NULLROUTER_STATE_FILE").map(PathBuf::from),
        }
    }
}

fn build_store(path: Option<&PathBuf>) -> std::io::Result<StateStore> {
    path.map_or_else(
        || Ok(StateStore::memory()),
        |path| StateStore::file(path).map_err(std::io::Error::other),
    )
}
