//! Minimal HTTP server used to drive execution tests against a real socket.
//!
//! Hand-rolled rather than pulled in as a dependency: the tests only need
//! fixed responses, recorded requests, and loopback binding.
//!
//! This is an integration-test helper, so the workspace's
//! `allow-expect-in-tests` (which only covers `#[cfg(test)]` code) does not
//! reach it; failing to bind a loopback socket should abort the test anyway.
#![allow(
    clippy::expect_used,
    clippy::missing_panics_doc,
    unreachable_pub,
    dead_code,
    reason = "test-only helper: panicking on setup failure is correct, and not every helper is used by every test file"
)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// One canned response.
#[derive(Debug, Clone)]
pub struct MockResponse {
    pub status: u16,
    pub content_type: String,
    pub body: String,
}

impl MockResponse {
    pub fn json(status: u16, body: &str) -> Self {
        Self {
            status,
            content_type: "application/json".to_owned(),
            body: body.to_owned(),
        }
    }

    pub fn sse(body: &str) -> Self {
        Self {
            status: 200,
            content_type: "text/event-stream".to_owned(),
            body: body.to_owned(),
        }
    }
}

/// A request the server observed.
#[derive(Debug, Clone)]
pub struct RecordedRequest {
    /// The HTTP method, so a poll (GET) is distinguishable from a create (POST).
    pub method: String,
    pub path: String,
    pub headers: BTreeMap<String, String>,
    pub body: String,
}

/// A loopback server that replies with queued responses in order.
#[derive(Debug)]
pub struct MockUpstream {
    pub addr: std::net::SocketAddr,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
}

impl MockUpstream {
    /// Bind on an ephemeral loopback port and serve `responses` in order.
    ///
    /// Once exhausted, the last response repeats.
    pub async fn start(responses: Vec<MockResponse>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback listener");
        let addr = listener.local_addr().expect("local addr");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);

        tokio::spawn(async move {
            let mut served = 0_usize;
            while let Ok((stream, _)) = listener.accept().await {
                let response = responses
                    .get(served)
                    .or_else(|| responses.last())
                    .cloned()
                    .unwrap_or_else(|| MockResponse::json(200, "{}"));
                served += 1;
                let sink = Arc::clone(&recorded);
                tokio::spawn(async move {
                    let _ = handle_connection(stream, response, sink).await;
                });
            }
        });

        Self { addr, requests }
    }

    /// Base URL for this server.
    pub fn url(&self) -> String {
        format!("http://{}/v1/chat/completions", self.addr)
    }

    /// Everything observed so far.
    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.requests
            .lock()
            .map(|requests| requests.clone())
            .unwrap_or_default()
    }

    /// How many requests arrived.
    pub fn request_count(&self) -> usize {
        self.requests.lock().map_or(0, |requests| requests.len())
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    response: MockResponse,
    recorded: Arc<Mutex<Vec<RecordedRequest>>>,
) -> std::io::Result<()> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 4096];

    // Read until headers are complete, then read exactly Content-Length bytes.
    let (headers_end, content_length) = loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break (buffer.len(), 0);
        }
        buffer.extend_from_slice(chunk.get(..read).unwrap_or_default());
        if let Some(position) = find_subsequence(&buffer, b"\r\n\r\n") {
            let head = String::from_utf8_lossy(buffer.get(..position).unwrap_or_default());
            let length = head
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())?
                })
                .unwrap_or(0);
            break (position + 4, length);
        }
    };

    while buffer.len() < headers_end + content_length {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(chunk.get(..read).unwrap_or_default());
    }

    let raw = String::from_utf8_lossy(&buffer).into_owned();
    let head = raw.get(..headers_end.min(raw.len())).unwrap_or_default();
    let mut lines = head.lines();
    let request_line = lines.next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("GET").to_owned();
    let path = parts.next().unwrap_or("/").to_owned();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.to_lowercase(), value.trim().to_owned()))
        .collect();
    let body = raw.get(headers_end..).unwrap_or_default().to_owned();

    if let Ok(mut sink) = recorded.lock() {
        sink.push(RecordedRequest {
            method,
            path,
            headers,
            body,
        });
    }

    let payload = format!(
        "HTTP/1.1 {} OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        response.status,
        response.content_type,
        response.body.len(),
        response.body
    );
    stream.write_all(payload.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
