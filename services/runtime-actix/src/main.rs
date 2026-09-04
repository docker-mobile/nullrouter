use actix_web::{App, HttpServer, web};
use nullrouter_runtime::{DEFAULT_HOST, DEFAULT_PORT, Runtime, app_config, configure};

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    nullrouter_logship::install("nullrouter-runtime");
    let server = ServerConfig::from_env();

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
    // TCP_NODELAY on every accepted socket. Without it a streamed response costs a
    // client roughly *twice* its real latency.
    //
    // The response goes out as a small header write followed by the body. On a
    // freshly-opened connection Linux is in quickack mode and ACKs at once, so nothing
    // stalls -- which is why the first request on a connection looks fine and hides this.
    // Once the connection settles into delayed ACK, Nagle holds the body behind the
    // unacknowledged header segment until the client's ACK timer fires, ~40 ms later.
    //
    // Measured on loopback with a raw socket: headers at 45 ms, then nothing for 42 ms,
    // then the whole body at once. Streamed p50 was 80 ms with client keep-alive and
    // 40 ms without. The Node-based router this replaces showed 49 ms either way, because
    // Node sets nodelay on HTTP sockets by default and so never had the stall. Real clients
    // all use keep-alive, so this was the case that mattered and the only one the
    // benchmark's non-streaming cells could not see.
    .on_connect(|connection, _extensions| {
        if let Some(stream) = connection.downcast_ref::<actix_web::rt::net::TcpStream>() {
            // A failure here costs latency, not correctness, so it is not fatal.
            let _ = stream.set_nodelay(true);
        }
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
