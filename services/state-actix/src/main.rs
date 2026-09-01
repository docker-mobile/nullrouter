use std::path::PathBuf;

use actix_web::{App, HttpServer, web};
use nullrouter_state::{DEFAULT_HOST, DEFAULT_PORT, StateStore, configure};

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    nullrouter_logship::install("nullrouter-state");
    let server = ServerConfig::from_env();
    let store = build_store(server.state_file.as_ref())?;

    // The two hottest mutations — advancing the round-robin cursor and appending a usage row —
    // defer their disk write, so something has to perform it. See `StoreInner::dirty`.
    let flusher = store.clone();
    let flush_task = actix_web::rt::spawn(async move {
        let mut ticker = actix_web::rt::time::interval(nullrouter_state::FLUSH_INTERVAL);
        loop {
            ticker.tick().await;
            if let Err(error) = flusher.flush_if_dirty() {
                // Logged rather than fatal: the flag is put back, so the next tick retries. A
                // permanently unwritable state file will say so once per tick, which is the
                // intended noise level for "your state is not being saved".
                tracing::error!(%error, "could not flush state to disk");
            }
        }
    });

    let running = HttpServer::new({
        let store = store.clone();
        move || {
            App::new()
                .app_data(web::Data::new(store.clone()))
                .configure(configure)
        }
    })
    .bind((server.host.as_str(), server.port))?
    .run()
    .await;

    // Stop ticking, then write whatever the last tick did not: a clean shutdown must not lose the
    // usage rows recorded in its final 250ms.
    flush_task.abort();
    if let Err(error) = store.flush_if_dirty() {
        tracing::error!(%error, "could not flush state to disk on shutdown");
    }
    running
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
