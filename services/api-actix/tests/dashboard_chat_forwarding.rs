//! The dashboard's basic-chat endpoint forwards to `nullrouter-runtime`.
//!
//! The runtime owns provider execution, so this endpoint must relay rather than
//! reimplement it — including passing SSE through unchanged.

#![allow(
    clippy::future_not_send,
    clippy::expect_used,
    reason = "test helper: failing to bind a loopback socket should abort the test"
)]

use std::sync::{Arc, Mutex};

use actix_web::{
    App,
    body::to_bytes,
    http::{Method, StatusCode, header},
    test, web,
};
use nullrouter_api::{AppConfig, RuntimeClient, StateClient, configure};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const UNREACHABLE_STATE_ADDR: &str = "127.0.0.1:1";

const fn app_config() -> AppConfig {
    AppConfig::new("0.5.20")
}

/// A stand-in runtime that returns a fixed reply and records what it received.
async fn fake_runtime(
    content_type: &'static str,
    body: &'static str,
) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("addr").to_string();
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&seen);

    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let sink = Arc::clone(&recorded);
            tokio::spawn(async move {
                let _ = serve(stream, content_type, body, sink).await;
            });
        }
    });

    (addr, seen)
}

async fn serve(
    mut stream: TcpStream,
    content_type: &str,
    reply: &str,
    seen: Arc<Mutex<Vec<String>>>,
) -> std::io::Result<()> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 4096];

    let (head_end, content_length) = loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break (buffer.len(), 0);
        }
        buffer.extend_from_slice(chunk.get(..read).unwrap_or_default());
        if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
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
    while buffer.len() < head_end + content_length {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(chunk.get(..read).unwrap_or_default());
    }

    let raw = String::from_utf8_lossy(&buffer).into_owned();
    let path = raw
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/")
        .to_owned();
    let body = raw.get(head_end..).unwrap_or_default().to_owned();

    if let Ok(mut sink) = seen.lock() {
        sink.push(format!("{path} {body}"));
    }

    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        content_type,
        reply.len(),
        reply
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await
}

struct Response {
    status: StatusCode,
    content_type: String,
    body: String,
}

async fn post_dashboard_chat(runtime_addr: &str, payload: &str) -> TestResult<Response> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(StateClient::new(UNREACHABLE_STATE_ADDR)))
            .app_data(web::Data::new(RuntimeClient::new(runtime_addr)))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::default()
        .method(Method::POST)
        .uri("/api/dashboard/chat/completions")
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(payload.to_owned())
        .to_request();
    let res = test::call_service(&app, req).await;
    let status = res.status();
    let content_type = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = String::from_utf8(to_bytes(res.into_body()).await?.to_vec())?;
    Ok(Response {
        status,
        content_type,
        body,
    })
}

#[actix_rt::test]
async fn dashboard_chat_relays_the_runtime_json_reply() -> TestResult {
    let (addr, seen) = fake_runtime(
        "application/json",
        r#"{"id":"chatcmpl-1","choices":[{"message":{"role":"assistant","content":"hello"}}]}"#,
    )
    .await;

    let response = post_dashboard_chat(
        &addr,
        r#"{"model":"openai/gpt-5","messages":[{"role":"user","content":"hi"}]}"#,
    )
    .await?;

    // The runtime's reply is relayed verbatim.
    assert_eq!(response.status, StatusCode::OK, "{}", response.body);
    assert!(response.content_type.starts_with("application/json"));
    let json: Value = serde_json::from_str(&response.body)?;
    assert_eq!(
        json.pointer("/choices/0/message/content"),
        Some(&json!("hello"))
    );

    // And it reached the runtime's OpenAI-compatible endpoint with the body intact.
    let requests = seen.lock().map(|seen| seen.clone()).unwrap_or_default();
    let forwarded = requests.first().expect("runtime was called");
    assert!(
        forwarded.contains("/v1/chat/completions"),
        "got {forwarded}"
    );
    assert!(
        forwarded.contains("\"model\":\"openai/gpt-5\""),
        "got {forwarded}"
    );
    Ok(())
}

#[actix_rt::test]
async fn dashboard_chat_passes_sse_through_unchanged() -> TestResult {
    let (addr, _) = fake_runtime(
        "text/event-stream",
        "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n\n",
    )
    .await;

    let response = post_dashboard_chat(
        &addr,
        r#"{"model":"openai/gpt-5","stream":true,"messages":[{"role":"user","content":"hi"}]}"#,
    )
    .await?;

    // Streaming replies keep their framing and content type.
    assert_eq!(response.status, StatusCode::OK);
    assert!(
        response.content_type.starts_with("text/event-stream"),
        "got {}",
        response.content_type
    );
    assert!(response.body.contains("data: [DONE]"), "{}", response.body);
    Ok(())
}

#[actix_rt::test]
async fn dashboard_chat_relays_upstream_error_status() -> TestResult {
    let (addr, _) = fake_runtime(
        "application/json",
        r#"{"error":{"message":"No active credentials","type":"invalid_request_error"}}"#,
    )
    .await;

    let response = post_dashboard_chat(
        &addr,
        r#"{"model":"openai/gpt-5","messages":[{"role":"user","content":"hi"}]}"#,
    )
    .await?;

    // The stub answers 200; the point is that the error body is relayed rather
    // than replaced with a synthetic one.
    let json: Value = serde_json::from_str(&response.body)?;
    assert!(
        json.pointer("/error/message")
            .and_then(Value::as_str)
            .is_some_and(|message| message.contains("No active credentials")),
        "{}",
        response.body
    );
    Ok(())
}

#[actix_rt::test]
async fn dashboard_chat_validates_before_forwarding() -> TestResult {
    let (addr, seen) = fake_runtime("application/json", "{}").await;

    // A body with no model must be rejected locally.
    let response = post_dashboard_chat(&addr, r#"{"messages":[]}"#).await?;
    assert_eq!(
        response.status,
        StatusCode::BAD_REQUEST,
        "{}",
        response.body
    );

    // A body with no messages must also be rejected locally.
    let missing_messages = post_dashboard_chat(&addr, r#"{"model":"openai/gpt-5"}"#).await?;
    assert_eq!(missing_messages.status, StatusCode::BAD_REQUEST);

    // Neither reached the runtime.
    let requests = seen.lock().map(|seen| seen.clone()).unwrap_or_default();
    assert!(
        requests.is_empty(),
        "invalid requests must not be forwarded, saw {requests:?}"
    );
    Ok(())
}

#[actix_rt::test]
async fn dashboard_chat_reports_an_unreachable_runtime() -> TestResult {
    let response = post_dashboard_chat(
        "127.0.0.1:1",
        r#"{"model":"openai/gpt-5","messages":[{"role":"user","content":"hi"}]}"#,
    )
    .await?;

    // A down runtime is reported as a structured error, not a transport failure.
    assert_eq!(
        response.status,
        StatusCode::NOT_IMPLEMENTED,
        "{}",
        response.body
    );
    let json: Value = serde_json::from_str(&response.body)?;
    assert!(json.get("error").is_some(), "{}", response.body);
    Ok(())
}
