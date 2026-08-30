//! `POST /api/models/test`: does this model actually answer?
//!
//! The route existed and returned 501. It now dispatches a one-token completion through
//! `nullrouter-runtime` and reports the outcome with latency.
//!
//! The thing worth testing here is not the happy path. It is that every *failure* reaches
//! the user as the provider's own words. A dashboard that says "test failed" for a wrong key,
//! a wrong model name, and a rate limit alike gives a user nothing to act on, and those three
//! are the reasons anyone presses this button.
//!
//! Also covered: a 200 carrying no completion is reported as a failure. Some providers answer
//! 200 with an error object, and calling that a working model is the false pass this route
//! exists to prevent.

#![allow(
    clippy::future_not_send,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test assertions read clearer with direct expect than with error plumbing"
)]

use std::sync::{Arc, Mutex};

use actix_web::{
    App,
    body::to_bytes,
    http::{StatusCode, header},
    test, web,
};
use nullrouter_api::{AppConfig, RuntimeClient, StateClient, configure};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const UNREACHABLE: &str = "127.0.0.1:1";

const fn app_config() -> AppConfig {
    AppConfig::new("0.5.20")
}

/// A stand-in runtime that answers `/v1/chat/completions` with a fixed status and body.
struct FakeRuntime {
    addr: std::net::SocketAddr,
    bodies: Arc<Mutex<Vec<String>>>,
}

impl FakeRuntime {
    async fn start(status: u16, body: String) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let bodies = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&bodies);
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let body = body.clone();
                let sink = Arc::clone(&seen);
                tokio::spawn(async move {
                    let _ = serve(stream, status, body, sink).await;
                });
            }
        });
        Self { addr, bodies }
    }

    fn addr_string(&self) -> String {
        self.addr.to_string()
    }

    /// The request bodies the fake runtime received.
    fn requests(&self) -> Vec<String> {
        self.bodies
            .lock()
            .map(|seen| seen.clone())
            .unwrap_or_default()
    }
}

async fn serve(
    mut stream: TcpStream,
    status: u16,
    body: String,
    seen: Arc<Mutex<Vec<String>>>,
) -> std::io::Result<()> {
    let mut buffer = Vec::new();
    let mut chunk = vec![0_u8; 16_384];
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
    if let Ok(mut sink) = seen.lock() {
        sink.push(raw.get(head_end..).unwrap_or_default().to_owned());
    }

    let response = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await
}

async fn test_model(runtime_addr: &str, payload: String) -> TestResult<(StatusCode, Value)> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(StateClient::new(UNREACHABLE)))
            .app_data(web::Data::new(RuntimeClient::new(runtime_addr)))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::post()
        .uri("/api/models/test")
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(payload)
        .to_request();
    let res = test::call_service(&app, req).await;
    let status = res.status();
    let body = String::from_utf8(to_bytes(res.into_body()).await?.to_vec())?;
    Ok((status, serde_json::from_str(&body)?))
}

fn ok_completion() -> String {
    json!({
        "id": "chatcmpl-1",
        "object": "chat.completion",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "h" },
            "finish_reason": "length",
        }],
        "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 },
    })
    .to_string()
}

#[actix_web::test]
async fn a_working_model_reports_ok_with_latency() -> TestResult {
    let runtime = FakeRuntime::start(200, ok_completion()).await;
    let (status, body) = test_model(
        &runtime.addr_string(),
        json!({ "model": "openai/gpt-4o-mini" }).to_string(),
    )
    .await?;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], true, "{body}");
    assert_eq!(body["model"], "openai/gpt-4o-mini");
    assert!(body["latencyMs"].is_number(), "latency missing: {body}");
    assert_eq!(body["finishReason"], "length");
    assert_eq!(body["usage"]["total_tokens"], 2);
    // No longer 501, which is the acceptance criterion.
    assert!(body.get("unsupported").is_none(), "{body}");
    Ok(())
}

#[actix_web::test]
async fn the_probe_is_one_token_and_not_streamed() -> TestResult {
    // A test button must not spend real credits, and must return one JSON body to read a
    // result from. Asserted on the wire rather than trusted.
    let runtime = FakeRuntime::start(200, ok_completion()).await;
    test_model(
        &runtime.addr_string(),
        json!({ "model": "openai/gpt-4o-mini" }).to_string(),
    )
    .await?;

    let sent = runtime.requests();
    assert_eq!(sent.len(), 1, "expected one dispatch, got {}", sent.len());
    let sent: Value = serde_json::from_str(&sent[0])?;
    assert_eq!(sent["max_tokens"], 1, "{sent}");
    assert_eq!(sent["stream"], false, "{sent}");
    assert_eq!(sent["model"], "openai/gpt-4o-mini");
    Ok(())
}

#[actix_web::test]
async fn a_rejected_key_reports_the_providers_own_words() -> TestResult {
    let runtime = FakeRuntime::start(
        401,
        json!({"error": {"message": "Incorrect API key provided", "type": "invalid_request_error"}})
            .to_string(),
    )
    .await;
    let (status, body) = test_model(
        &runtime.addr_string(),
        json!({ "model": "openai/gpt-4o-mini" }).to_string(),
    )
    .await?;

    // The route answers 200 with ok:false: the *test* succeeded, the model did not.
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], false, "{body}");
    assert_eq!(body["status"], 401);
    assert_eq!(
        body["error"], "Incorrect API key provided",
        "the provider's message must survive verbatim: {body}"
    );
    Ok(())
}

#[actix_web::test]
async fn a_rate_limit_reports_the_providers_own_words() -> TestResult {
    let runtime = FakeRuntime::start(
        429,
        json!({"error": {"message": "Rate limit reached for gpt-4o-mini"}}).to_string(),
    )
    .await;
    let (_, body) = test_model(
        &runtime.addr_string(),
        json!({ "model": "openai/gpt-4o-mini" }).to_string(),
    )
    .await?;

    assert_eq!(body["ok"], false);
    assert_eq!(body["status"], 429);
    assert_eq!(body["error"], "Rate limit reached for gpt-4o-mini");
    Ok(())
}

#[actix_web::test]
async fn a_missing_model_reports_the_providers_own_words() -> TestResult {
    let runtime = FakeRuntime::start(
        404,
        json!({"error": {"message": "The model `nope` does not exist"}}).to_string(),
    )
    .await;
    let (_, body) = test_model(
        &runtime.addr_string(),
        json!({ "model": "openai/nope" }).to_string(),
    )
    .await?;

    assert_eq!(body["ok"], false);
    assert_eq!(body["error"], "The model `nope` does not exist");
    Ok(())
}

#[actix_web::test]
async fn a_two_hundred_with_no_completion_is_not_a_pass() -> TestResult {
    // Some providers answer 200 with an error object. Calling that a working model is the
    // false pass this route exists to prevent.
    let runtime = FakeRuntime::start(
        200,
        json!({"error": {"message": "quota exceeded"}}).to_string(),
    )
    .await;
    let (_, body) = test_model(
        &runtime.addr_string(),
        json!({ "model": "openai/gpt-4o-mini" }).to_string(),
    )
    .await?;

    assert_eq!(
        body["ok"], false,
        "a 200 with no completion must not pass: {body}"
    );
    assert_eq!(body["providerError"], "quota exceeded", "{body}");
    Ok(())
}

#[actix_web::test]
async fn an_empty_completion_with_a_finish_reason_still_passes() -> TestResult {
    // `max_tokens: 1` legitimately yields empty content with finish_reason "length". Calling
    // that a failure would report every working model as broken.
    let runtime = FakeRuntime::start(
        200,
        json!({
            "choices": [{ "index": 0, "message": {"role": "assistant", "content": ""},
                          "finish_reason": "length" }],
        })
        .to_string(),
    )
    .await;
    let (_, body) = test_model(
        &runtime.addr_string(),
        json!({ "model": "openai/gpt-4o-mini" }).to_string(),
    )
    .await?;

    assert_eq!(body["ok"], true, "{body}");
    Ok(())
}

#[actix_web::test]
async fn an_html_error_page_still_tells_the_user_something() -> TestResult {
    let runtime = FakeRuntime::start(
        502,
        "<html><head><title>502 Bad Gateway</title></head></html>".to_owned(),
    )
    .await;
    let (_, body) = test_model(
        &runtime.addr_string(),
        json!({ "model": "openai/gpt-4o-mini" }).to_string(),
    )
    .await?;

    assert_eq!(body["ok"], false);
    let error = body["error"].as_str().unwrap_or_default();
    assert!(error.contains("502 Bad Gateway"), "got {error:?}");
    Ok(())
}

#[actix_web::test]
async fn an_unreachable_runtime_is_reported_as_such() -> TestResult {
    // Distinguishable from a provider failure: the user's own router is down, which is a
    // different thing to fix.
    let (status, body) = test_model(
        UNREACHABLE,
        json!({ "model": "openai/gpt-4o-mini" }).to_string(),
    )
    .await?;

    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["ok"], false);
    let error = body["error"].as_str().unwrap_or_default();
    assert!(error.contains("runtime"), "got {error:?}");
    Ok(())
}

#[actix_web::test]
async fn a_missing_model_name_is_a_bad_request() -> TestResult {
    let runtime = FakeRuntime::start(200, ok_completion()).await;
    let (status, _) = test_model(&runtime.addr_string(), json!({}).to_string()).await?;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (status, _) = test_model(
        &runtime.addr_string(),
        json!({ "model": "   " }).to_string(),
    )
    .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(
        runtime.requests().len(),
        0,
        "a rejected request must not reach a provider"
    );
    Ok(())
}

#[actix_web::test]
async fn a_non_chat_kind_is_refused_before_dispatch() -> TestResult {
    // An embedding model would reject a chat body, and reporting that rejection as a
    // provider failure would send the user looking for a problem that is not there.
    let runtime = FakeRuntime::start(200, ok_completion()).await;
    let (status, body) = test_model(
        &runtime.addr_string(),
        json!({ "model": "openai/text-embedding-3-small", "kind": "embedding" }).to_string(),
    )
    .await?;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["ok"], false);
    assert_eq!(
        runtime.requests().len(),
        0,
        "an embedding model should not be sent a chat completion"
    );
    Ok(())
}
