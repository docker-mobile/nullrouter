use actix_web::{App, HttpServer, web};
use nullrouter_api::{
    AppConfig, RuntimeClient, ShutdownHandle, StateClient, TunnelManager, configure,
};

const DEFAULT_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 20129;

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    nullrouter_logship::install("nullrouter-api");
    let config = ServerConfig::from_env();
    let app_config = AppConfig::default();
    // Holds a connection pool, so it is built once and shared across workers.
    let state = web::Data::new(StateClient::from_env());
    let runtime = web::Data::new(RuntimeClient::from_env());
    // Reads the install directory and the event log. Shared rather than per-worker
    // so every worker reports the same install state.
    let token_saver = web::Data::new(nullrouter_pxpipe::TokenSaver::discover());
    // Registered empty and filled in after `run()`, because the handle does not exist until
    // the server is built. `/api/shutdown` reports that it cannot stop anything if this is
    // still empty, rather than claiming a shutdown that will not happen.
    let shutdown = web::Data::new(ShutdownHandle::new());
    // Built once, outside the per-worker closure: it owns the supervisor threads, and one
    // pair per worker would mean several cloudflared children with no single owner — the
    // exact confusion upstream's pid file plus `pkill` fallback exists to paper over.
    let tunnels = web::Data::new(TunnelManager::new());

    let server = {
        let shutdown = shutdown.clone();
        HttpServer::new(move || {
            App::new()
                .app_data(web::Data::new(app_config.clone()))
                // Without this the usage endpoints would fail at runtime, since
                // they extract `web::Data<StateClient>`.
                .app_data(state.clone())
                .app_data(runtime.clone())
                .app_data(token_saver.clone())
                .app_data(shutdown.clone())
                .app_data(tunnels.clone())
                .configure(configure)
        })
    // TCP_NODELAY, for the same reason as `nullrouter-runtime` — see the long note in
    // `services/runtime-actix/src/main.rs`.
    .on_connect(|connection, _extensions| {
        if let Some(stream) = connection.downcast_ref::<actix_web::rt::net::TcpStream>() {
            let _ = stream.set_nodelay(true);
        }
    })
        .bind((config.host.as_str(), config.port))?
        .run()
    };
    shutdown.set(server.handle());
    server.await
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
