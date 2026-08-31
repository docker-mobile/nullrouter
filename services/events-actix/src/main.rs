use actix_web::{App, HttpServer, web};
use nullrouter_events::{DEFAULT_HOST, DEFAULT_PORT, McpBridge, UsageReader, configure};

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    let server = ServerConfig::from_env();
    // Holds a connection pool, so it is built once and shared across workers.
    let usage = web::Data::new(UsageReader::from_env());
    // One bridge for the whole process, not one per worker: an MCP child is a process, and a
    // per-worker bridge would spawn N children for one plugin and reap only the ones its own
    // worker started.
    let mcp = McpBridge::new();
    let reaper = mcp.clone();

    let result = HttpServer::new(move || {
        let mcp = mcp.clone();
        App::new()
            // The live usage stream extracts this; without it the route would
            // fail at runtime.
            .app_data(usage.clone())
            // Before `configure`: it registers a default bridge only when none is present, so
            // registering after this would orphan every child from `reaper`.
            .configure(move |config| mcp.register(config))
            .configure(configure)
    })
    // TCP_NODELAY, for the same reason as `nullrouter-runtime` — see the long note in
    // `services/runtime-actix/src/main.rs`. This service is entirely SSE, so it is the one
    // most exposed to it: without nodelay, every frame after the headers can wait on the
    // client's delayed-ACK timer once the connection is warm.
    .on_connect(|connection, _extensions| {
        if let Some(stream) = connection.downcast_ref::<actix_web::rt::net::TcpStream>() {
            let _ = stream.set_nodelay(true);
        }
    })
    .bind((server.host.as_str(), server.port))?
    .run()
    .await;

    // After the server stops accepting, and on the way out of either outcome: a bind failure
    // returns earlier with `?`, so reaching here means children may exist. Not reaping them would
    // leave an `npx` process per plugin running with nothing to talk to.
    reaper.shutdown().await;
    result
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
