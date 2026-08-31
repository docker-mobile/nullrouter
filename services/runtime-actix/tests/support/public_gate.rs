//! A state stub that answers only the `/v1` admission gate.
//!
//! Several suites assert route shapes and refusals without running a state service: they point the
//! runtime at a closed port, and credential selection then fails deterministically. That stopped
//! working when admission became a live state call — every request answered "gate unavailable"
//! before reaching the behaviour under test, which is the correct production response and useless
//! as a test seam.
//!
//! This stub declares the gate public and refuses everything else with 404, so credential selection
//! still fails exactly as a closed port made it fail. The assertions those suites already carry are
//! preserved rather than rewritten to 503, which would have replaced route-shape coverage with
//! twelve copies of one fact about admission.

use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// Start the stub and return its `host:port`.
pub(crate) async fn start() -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("addr").to_string();
    let requests = Arc::new(());
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let _keep = Arc::clone(&requests);
            tokio::spawn(async move {
                let mut buffer = [0_u8; 8192];
                let read = stream.read(&mut buffer).await.unwrap_or(0);
                let head = String::from_utf8_lossy(buffer.get(..read).unwrap_or_default());
                let response = if head.contains("/internal/v1/keys/gate") {
                    let body = r#"{"requireApiKey":false,"valid":false,"active":false}"#;
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                } else {
                    // Everything else is absent, so selection fails as it did against a closed port.
                    "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .to_owned()
                };
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.flush().await;
            });
        }
    });
    addr
}
