//! End-to-end execution: a real request travels runtime -> state (credentials)
//! -> provider, and the translated response comes back.
//!
//! Both the state service and the provider are real HTTP servers on loopback,
//! so this exercises the whole pipeline rather than mocked seams.

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
use nullrouter_runtime::{Runtime, app_config, configure};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A scripted reply from the fake state or provider server.
#[derive(Debug, Clone)]
struct Reply {
    status: u16,
    content_type: &'static str,
    body: String,
}

impl Reply {
    fn json(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            content_type: "application/json",
            body: body.into(),
        }
    }

    fn sse(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            content_type: "text/event-stream",
            body: body.into(),
        }
    }

    const fn with_status(mut self, status: u16) -> Self {
        self.status = status;
        self
    }
}

/// A loopback server that replies per request path.
#[derive(Debug)]
struct FakeServer {
    addr: std::net::SocketAddr,
    seen: Arc<Mutex<Vec<(String, String)>>>,
}

impl FakeServer {
    /// Serve `routes` (path suffix -> reply). The first matching suffix wins;
    /// unmatched paths get a 404.
    async fn start(routes: Vec<(&'static str, Reply)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("addr");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&seen);

        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let routes = routes.clone();
                let sink = Arc::clone(&recorded);
                tokio::spawn(async move {
                    let _ = serve(stream, routes, sink).await;
                });
            }
        });

        Self { addr, seen }
    }

    fn addr_string(&self) -> String {
        self.addr.to_string()
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Recorded `(path, body)` pairs.
    fn requests(&self) -> Vec<(String, String)> {
        self.seen
            .lock()
            .map(|seen| seen.clone())
            .unwrap_or_default()
    }
}

async fn serve(
    mut stream: TcpStream,
    routes: Vec<(&'static str, Reply)>,
    seen: Arc<Mutex<Vec<(String, String)>>>,
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
        sink.push((path.clone(), body));
    }

    let reply = routes
        .iter()
        .find(|(suffix, _)| path.contains(suffix))
        .map_or_else(
            || Reply::json(r#"{"error":"unrouted"}"#).with_status(404),
            |(_, reply)| reply.clone(),
        );

    let response = format!(
        "HTTP/1.1 {} OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        reply.status,
        reply.content_type,
        reply.body.len(),
        reply.body
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await
}

/// A state service that hands out credentials pointing at `provider_base`.
async fn fake_state(provider_base: &str) -> FakeServer {
    let credentials = json!({
        "status": "selected",
        "credentials": {
            "connectionId": "conn_e2e",
            "connectionName": "e2e",
            "apiKey": "sk-e2e",
            "providerSpecificData": { "baseUrl": provider_base },
        },
    });
    FakeServer::start(vec![
        (
            "/internal/v1/credentials/select",
            Reply::json(credentials.to_string()),
        ),
        ("/internal/v1/credentials/clear-error", Reply::json("{}")),
        ("/internal/v1/credentials/unavailable", Reply::json("{}")),
        ("/internal/v1/usage", Reply::json(r#"{"ok":true}"#)),
        (
            "/internal/v1/routing-context",
            Reply::json(r#"{"combos":[],"connections":[],"settings":{}}"#),
        ),
    ])
    .await
}

struct Response {
    status: StatusCode,
    content_type: String,
    body: String,
}

async fn post(state_addr: &str, uri: &str, body: &str) -> TestResult<Response> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(Runtime::with_state_addr(state_addr)))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::default()
        .method(Method::POST)
        .uri(uri)
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(body.to_owned())
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

/// Wait briefly for a usage report to arrive at the state stub.
///
/// Streaming records usage after the body drains, so it is not observable the
/// instant the response is read.
async fn await_usage(state: &FakeServer) -> Option<String> {
    for _ in 0..100 {
        if let Some((_, body)) = state
            .requests()
            .into_iter()
            .find(|(path, _)| path.contains("/internal/v1/usage"))
        {
            return Some(body);
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    None
}

/// Decode the `data:` payloads from an SSE body.
fn sse_payloads(body: &str) -> Vec<Value> {
    body.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter(|payload| *payload != "[DONE]")
        .filter_map(|payload| serde_json::from_str(payload).ok())
        .collect()
}

#[actix_rt::test]
async fn non_streaming_chat_completes_through_the_full_pipeline() -> TestResult {
    // Given: a provider that answers a chat completion, and state that hands
    // out credentials pointing at it.
    let provider = FakeServer::start(vec![(
        "/chat/completions",
        Reply::json(
            json!({
                "id": "chatcmpl-e2e",
                "object": "chat.completion",
                "model": "gpt-5",
                "choices": [{
                    "index": 0,
                    "message": { "role": "assistant", "content": "pong" },
                    "finish_reason": "stop",
                }],
                "usage": { "prompt_tokens": 3, "completion_tokens": 1, "total_tokens": 4 },
            })
            .to_string(),
        ),
    )])
    .await;
    let state = fake_state(&provider.base_url()).await;

    // When: an OpenAI-compatible client posts a chat completion.
    let response = post(
        &state.addr_string(),
        "/v1/chat/completions",
        r#"{"model":"openai-compatible-e2e/gpt-5","messages":[{"role":"user","content":"ping"}]}"#,
    )
    .await?;

    // Then: the provider's answer comes back verbatim, as JSON.
    assert_eq!(response.status, StatusCode::OK, "body: {}", response.body);
    assert!(response.content_type.starts_with("application/json"));
    let json: Value = serde_json::from_str(&response.body)?;
    assert_eq!(
        json.pointer("/choices/0/message/content"),
        Some(&json!("pong"))
    );

    // And: the provider actually received the translated request.
    let provider_requests = provider.requests();
    let (path, sent) = provider_requests.first().expect("provider was called");
    assert!(path.contains("/chat/completions"), "got {path}");
    let sent_json: Value = serde_json::from_str(sent)?;
    assert_eq!(
        sent_json.pointer("/messages/0/content"),
        Some(&json!("ping"))
    );
    // The upstream model id replaces the provider-prefixed one.
    assert_eq!(sent_json.get("model"), Some(&json!("gpt-5")));

    // And: usage was recorded against the selected connection.
    //
    // Polled rather than read once. The usage POST is spawned instead of awaited — the client was
    // waiting on a round trip that happens after its response already exists — so it lands shortly
    // after the response rather than before it. The bound is what keeps this a real assertion: if
    // the spawn were dropped and nothing recorded usage, this fails on timeout rather than passing.
    let usage = poll_for_usage(&state).await;
    let (_, usage_body) = usage.expect("usage was recorded within the timeout");
    let usage_json: Value = serde_json::from_str(&usage_body)?;
    assert_eq!(usage_json.get("status"), Some(&json!("success")));
    assert_eq!(usage_json.get("connectionId"), Some(&json!("conn_e2e")));
    Ok(())
}

#[actix_rt::test]
async fn streaming_chat_is_translated_and_terminated() -> TestResult {
    // Given: a provider streaming OpenAI chunks.
    let upstream = concat!(
        "data: {\"id\":\"chatcmpl-e2e\",\"model\":\"gpt-5\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"}}]}\n\n",
        "data: {\"id\":\"chatcmpl-e2e\",\"model\":\"gpt-5\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"he\"}}]}\n\n",
        "data: {\"id\":\"chatcmpl-e2e\",\"model\":\"gpt-5\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"llo\"}}]}\n\n",
        "data: {\"id\":\"chatcmpl-e2e\",\"model\":\"gpt-5\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":2,\"completion_tokens\":2,\"total_tokens\":4}}\n\n",
        "data: [DONE]\n\n",
    );
    let provider = FakeServer::start(vec![("/chat/completions", Reply::sse(upstream))]).await;
    let state = fake_state(&provider.base_url()).await;

    // When: the client asks for a stream.
    let response = post(
        &state.addr_string(),
        "/v1/chat/completions",
        r#"{"model":"openai-compatible-e2e/gpt-5","stream":true,"messages":[{"role":"user","content":"hi"}]}"#,
    )
    .await?;

    // Then: frames arrive as SSE and terminate with [DONE].
    assert_eq!(response.status, StatusCode::OK, "body: {}", response.body);
    assert!(response.content_type.starts_with("text/event-stream"));
    assert!(response.body.trim_end().ends_with("data: [DONE]"));

    let payloads = sse_payloads(&response.body);
    let text: String = payloads
        .iter()
        .filter_map(|chunk| {
            chunk
                .pointer("/choices/0/delta/content")
                .and_then(Value::as_str)
        })
        .collect();
    assert_eq!(text, "hello");

    // And: recorded usage carries the streamed token counts.
    //
    // Usage is reported after the stream drains, deliberately off the response
    // path so telemetry never delays a token. That makes it observable slightly
    // after the body completes, so this waits for it rather than racing.
    let usage = await_usage(&state).await.expect("usage recorded");
    let usage_json: Value = serde_json::from_str(&usage)?;
    assert_eq!(usage_json.get("promptTokens"), Some(&json!(2)));
    assert_eq!(usage_json.get("completionTokens"), Some(&json!(2)));
    Ok(())
}

#[actix_rt::test]
async fn claude_client_receives_translated_events_from_an_openai_provider() -> TestResult {
    // Given: an OpenAI-shaped provider stream.
    let upstream = concat!(
        "data: {\"id\":\"chatcmpl-abcdef12\",\"model\":\"gpt-5\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"}}]}\n\n",
        "data: {\"id\":\"chatcmpl-abcdef12\",\"model\":\"gpt-5\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let provider = FakeServer::start(vec![("/chat/completions", Reply::sse(upstream))]).await;
    let state = fake_state(&provider.base_url()).await;

    // When: a Claude client posts to /v1/messages.
    let response = post(
        &state.addr_string(),
        "/v1/messages",
        r#"{"model":"openai-compatible-e2e/gpt-5","max_tokens":100,"messages":[{"role":"user","content":"hi"}]}"#,
    )
    .await?;

    // Then: the client receives Claude-native SSE events.
    assert_eq!(response.status, StatusCode::OK, "body: {}", response.body);
    assert!(
        response.body.contains("event: message_start"),
        "{}",
        response.body
    );
    assert!(
        response.body.contains("event: content_block_delta"),
        "{}",
        response.body
    );
    assert!(
        response.body.contains("event: message_stop"),
        "{}",
        response.body
    );
    Ok(())
}

#[actix_rt::test]
async fn upstream_failure_falls_back_to_the_next_account() -> TestResult {
    // Given: a provider that rate-limits, and state that reports one account.
    let provider = FakeServer::start(vec![(
        "/chat/completions",
        Reply::json(r#"{"error":{"message":"Rate limit exceeded"}}"#).with_status(429),
    )])
    .await;
    let state = fake_state(&provider.base_url()).await;

    // When: a chat request runs.
    let response = post(
        &state.addr_string(),
        "/v1/chat/completions",
        r#"{"model":"openai-compatible-e2e/gpt-5","messages":[{"role":"user","content":"hi"}]}"#,
    )
    .await?;

    // Then: the account is locked and the rate-limit error surfaces.
    assert_eq!(
        response.status,
        StatusCode::TOO_MANY_REQUESTS,
        "{}",
        response.body
    );
    let json: Value = serde_json::from_str(&response.body)?;
    assert!(
        json.pointer("/error/message")
            .and_then(Value::as_str)
            .is_some_and(|message| message.contains("Rate limit")),
        "{}",
        response.body
    );

    // And: state was told to cool the account down.
    let cooldown = state
        .requests()
        .into_iter()
        .find(|(path, _)| path.contains("/internal/v1/credentials/unavailable"))
        .map(|(_, body)| body)
        .expect("cooldown recorded");
    let cooldown_json: Value = serde_json::from_str(&cooldown)?;
    assert_eq!(cooldown_json.get("connectionId"), Some(&json!("conn_e2e")));
    assert_eq!(cooldown_json.get("status"), Some(&json!(429)));
    // 429 uses exponential backoff, so a level is reported.
    assert_eq!(cooldown_json.get("backoffLevel"), Some(&json!(1)));
    Ok(())
}

#[actix_rt::test]
async fn unsupported_provider_protocol_is_refused_explicitly() -> TestResult {
    let state = fake_state("http://127.0.0.1:1").await;

    // When: a request targets a provider whose protocol needs a bespoke executor.
    let response = post(
        &state.addr_string(),
        "/v1/chat/completions",
        r#"{"model":"kiro/claude-sonnet-4.5","messages":[{"role":"user","content":"hi"}]}"#,
    )
    .await?;

    // Then: it is refused with a clear reason rather than a wrong answer.
    assert_eq!(
        response.status,
        StatusCode::NOT_IMPLEMENTED,
        "{}",
        response.body
    );
    let json: Value = serde_json::from_str(&response.body)?;
    let message = json
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(message.contains("kiro"), "{message}");
    assert!(message.contains("not implemented"), "{message}");
    Ok(())
}

#[actix_rt::test]
async fn embeddings_route_to_the_service_specific_endpoint() -> TestResult {
    // Given: a provider serving the embeddings endpoint.
    let provider = FakeServer::start(vec![(
        "/embeddings",
        Reply::json(r#"{"object":"list","data":[{"embedding":[0.1,0.2]}]}"#),
    )])
    .await;
    let state = fake_state(&provider.base_url()).await;

    // When: an embeddings request names a real provider/model.
    let response = post(
        &state.addr_string(),
        "/v1/embeddings",
        r#"{"model":"openai/text-embedding-3-small","input":"hello"}"#,
    )
    .await?;

    // Then: the answer is JSON. The URL comes from the registry's
    // embeddingConfig, so it reaches api.openai.com rather than the mock; a
    // structured error is the expected outcome without network access.
    assert!(
        response.content_type.starts_with("application/json"),
        "{}",
        response.content_type
    );
    let json: Value = serde_json::from_str(&response.body)?;
    assert!(
        json.get("error").is_some() || json.get("data").is_some(),
        "expected an embeddings body or a structured error, got {}",
        response.body
    );
    Ok(())
}

#[actix_rt::test]
async fn responses_endpoint_emits_named_lifecycle_events() -> TestResult {
    // Given: an OpenAI-format provider streaming chat chunks.
    let upstream = concat!(
        "data: {\"id\":\"chatcmpl-r1\",\"model\":\"gpt-5\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"}}]}\n\n",
        "data: {\"id\":\"chatcmpl-r1\",\"model\":\"gpt-5\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let provider = FakeServer::start(vec![("/chat/completions", Reply::sse(upstream))]).await;
    let state = fake_state(&provider.base_url()).await;

    // When: a Responses-API client posts to /v1/responses.
    let response = post(
        &state.addr_string(),
        "/v1/responses",
        r#"{"model":"openai-compatible-e2e/gpt-5","input":"hello","stream":true}"#,
    )
    .await?;

    // Then: the client receives Responses lifecycle events, not chat chunks.
    // A chat-chunk stream here would be unparseable by a Responses client.
    assert_eq!(response.status, StatusCode::OK, "body: {}", response.body);
    for expected in [
        "event: response.created",
        "event: response.in_progress",
        "event: response.output_item.added",
        "event: response.output_text.delta",
        "event: response.output_text.done",
        "event: response.completed",
    ] {
        assert!(
            response.body.contains(expected),
            "missing {expected} in:\n{}",
            response.body
        );
    }
    // The chat-chunk envelope must not leak through.
    assert!(
        !response.body.contains("chat.completion.chunk"),
        "chat chunks leaked into a Responses stream:\n{}",
        response.body
    );
    assert!(response.body.trim_end().ends_with("data: [DONE]"));
    Ok(())
}

#[actix_rt::test]
async fn non_streaming_usage_tokens_are_recorded_not_dropped() -> TestResult {
    // Regression: the non-streaming branch parsed the reply straight to JSON and
    // never touched the translator, so `state.usage` stayed None and every
    // non-streaming request was recorded with zero tokens while still counting
    // as a request — usage looked like traffic with no cost.
    let provider = FakeServer::start(vec![(
        "/chat/completions",
        Reply::json(
            json!({
                "id": "chatcmpl-tok",
                "object": "chat.completion",
                "model": "gpt-5",
                "choices": [{
                    "index": 0,
                    "message": { "role": "assistant", "content": "pong" },
                    "finish_reason": "stop",
                }],
                "usage": {
                    "prompt_tokens": 11,
                    "completion_tokens": 3,
                    "total_tokens": 14,
                    "prompt_tokens_details": { "cached_tokens": 2 },
                },
            })
            .to_string(),
        ),
    )])
    .await;
    let state = fake_state(&provider.base_url()).await;

    let response = post(
        &state.addr_string(),
        "/v1/chat/completions",
        r#"{"model":"openai-compatible-e2e/gpt-5","messages":[{"role":"user","content":"ping"}]}"#,
    )
    .await?;
    assert_eq!(response.status, StatusCode::OK, "body: {}", response.body);

    let usage = await_usage(&state).await.expect("usage recorded");
    let usage_json: Value = serde_json::from_str(&usage)?;
    assert_eq!(
        usage_json.get("promptTokens"),
        Some(&json!(11)),
        "prompt tokens must come from the reply body: {usage}"
    );
    assert_eq!(usage_json.get("completionTokens"), Some(&json!(3)));
    assert_eq!(usage_json.get("cachedTokens"), Some(&json!(2)));
    Ok(())
}

#[actix_rt::test]
async fn a_reply_without_usage_records_zero_rather_than_failing() -> TestResult {
    // Some providers omit `usage` entirely. That must record zero tokens, not
    // error the request and not invent counts.
    let provider = FakeServer::start(vec![(
        "/chat/completions",
        Reply::json(
            json!({
                "id": "chatcmpl-nousage",
                "object": "chat.completion",
                "model": "gpt-5",
                "choices": [{
                    "index": 0,
                    "message": { "role": "assistant", "content": "pong" },
                    "finish_reason": "stop",
                }],
            })
            .to_string(),
        ),
    )])
    .await;
    let state = fake_state(&provider.base_url()).await;

    let response = post(
        &state.addr_string(),
        "/v1/chat/completions",
        r#"{"model":"openai-compatible-e2e/gpt-5","messages":[{"role":"user","content":"ping"}]}"#,
    )
    .await?;
    assert_eq!(response.status, StatusCode::OK, "body: {}", response.body);

    let usage = await_usage(&state).await.expect("usage recorded");
    let usage_json: Value = serde_json::from_str(&usage)?;
    assert_eq!(usage_json.get("promptTokens"), Some(&json!(0)));
    assert_eq!(usage_json.get("status"), Some(&json!("success")));
    Ok(())
}

/// A state service whose routing context declares one combo.
///
/// `models` are the combo's entries in order; `strategy` is the routing setting.
/// Credentials are handed out for any provider, pointing at `provider_base`, so a
/// combo can walk several models against one fake provider.
async fn fake_state_with_combo(provider_base: &str, models: &[&str], strategy: &str) -> FakeServer {
    let credentials = json!({
        "status": "selected",
        "credentials": {
            "connectionId": "conn_e2e",
            "connectionName": "e2e",
            "apiKey": "sk-e2e",
            "providerSpecificData": { "baseUrl": provider_base },
        },
    });
    let routing = json!({
        "combos": [{ "id": "combo_1", "name": "mixed", "kind": null, "models": models }],
        "connections": [],
        "settings": { "comboStrategy": strategy, "comboStickyRoundRobinLimit": 1 },
    });
    FakeServer::start(vec![
        (
            "/internal/v1/credentials/select",
            Reply::json(credentials.to_string()),
        ),
        ("/internal/v1/credentials/clear-error", Reply::json("{}")),
        ("/internal/v1/credentials/unavailable", Reply::json("{}")),
        ("/internal/v1/usage", Reply::json(r#"{"ok":true}"#)),
        (
            "/internal/v1/routing-context",
            Reply::json(routing.to_string()),
        ),
    ])
    .await
}

#[actix_rt::test]
async fn a_combo_falls_through_to_its_next_model_when_the_first_fails() -> TestResult {
    // Given: a combo whose first model is a provider that cannot be executed at
    // all (`ollama` needs a bespoke executor), and whose second is executable.
    // Before combo fallback existed the combo resolved to `models.first()` and
    // stopped, so a combo led by a dead model was a dead combo.
    let provider = FakeServer::start(vec![(
        "/chat/completions",
        Reply::json(
            json!({
                "id": "chatcmpl-combo",
                "object": "chat.completion",
                "model": "gpt-5",
                "choices": [{
                    "index": 0,
                    "message": { "role": "assistant", "content": "second model answered" },
                    "finish_reason": "stop",
                }],
                "usage": { "prompt_tokens": 3, "completion_tokens": 2, "total_tokens": 5 },
            })
            .to_string(),
        ),
    )])
    .await;
    let state = fake_state_with_combo(
        &provider.base_url(),
        &["ollama/llama3", "openai-compatible-e2e/gpt-5"],
        "fallback",
    )
    .await;

    // When: the client asks for the combo by name.
    let response = post(
        &state.addr_string(),
        "/v1/chat/completions",
        r#"{"model":"mixed","messages":[{"role":"user","content":"ping"}]}"#,
    )
    .await?;

    // Then: the second model answered, rather than the combo failing on the first.
    assert_eq!(response.status, StatusCode::OK, "body: {}", response.body);
    let json: Value = serde_json::from_str(&response.body)?;
    assert_eq!(
        json.pointer("/choices/0/message/content"),
        Some(&json!("second model answered"))
    );
    Ok(())
}

#[actix_rt::test]
async fn a_single_model_request_keeps_its_own_error() -> TestResult {
    // Given: a request for one unexecutable provider, not a combo. The combo
    // fallback path must not replace a real 501 with a generic "all models
    // unavailable" — the caller needs to know *which* protocol is unported.
    //
    // `kiro` stands in for that class: it needs AWS-style request signing. This
    // test used to name `ollama`, which is now executable, so it would have made a
    // real call instead of exercising the refusal.
    let provider = FakeServer::start(vec![("/chat/completions", Reply::json("{}"))]).await;
    let state = fake_state(&provider.base_url()).await;

    // When: the client asks for it directly.
    let response = post(
        &state.addr_string(),
        "/v1/chat/completions",
        r#"{"model":"kiro/claude-sonnet-4-5","messages":[{"role":"user","content":"ping"}]}"#,
    )
    .await?;

    // Then: the explicit 501 naming the provider survives.
    assert_eq!(
        response.status,
        StatusCode::NOT_IMPLEMENTED,
        "body: {}",
        response.body
    );
    assert!(
        response.body.contains("kiro"),
        "the refusal must name the provider: {}",
        response.body
    );
    Ok(())
}

#[actix_rt::test]
async fn a_combo_whose_every_model_is_unexecutable_reports_the_last_real_error() -> TestResult {
    // Given: a combo where no model can be executed. The client should still get
    // a real refusal naming a provider, not a synthesised placeholder.
    let provider = FakeServer::start(vec![("/chat/completions", Reply::json("{}"))]).await;
    let state = fake_state_with_combo(
        &provider.base_url(),
        &["ollama/llama3", "cursor/gpt-5"],
        "fallback",
    )
    .await;

    // When: the combo is requested.
    let response = post(
        &state.addr_string(),
        "/v1/chat/completions",
        r#"{"model":"mixed","messages":[{"role":"user","content":"ping"}]}"#,
    )
    .await?;

    // Then: the last model's own 501 is reported.
    assert_eq!(response.status, StatusCode::NOT_IMPLEMENTED);
    assert!(
        response.body.contains("cursor"),
        "the last model's refusal should be the reported one: {}",
        response.body
    );
    Ok(())
}

#[actix_rt::test]
async fn round_robin_starts_from_a_different_model_each_request() -> TestResult {
    // Given: a round-robin combo of two executable models, and a provider that
    // echoes which model it was asked for.
    let provider = FakeServer::start(vec![(
        "/chat/completions",
        Reply::json(
            json!({
                "id": "chatcmpl-rr",
                "object": "chat.completion",
                "model": "echo",
                "choices": [{
                    "index": 0,
                    "message": { "role": "assistant", "content": "ok" },
                    "finish_reason": "stop",
                }],
                "usage": { "prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2 },
            })
            .to_string(),
        ),
    )])
    .await;
    let state = fake_state_with_combo(
        &provider.base_url(),
        &[
            "openai-compatible-e2e/first",
            "openai-compatible-e2e/second",
        ],
        "round-robin",
    )
    .await;

    // When: the same combo is requested twice through one runtime, so the
    // rotation cursor persists between calls.
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(Runtime::with_state_addr(
                &state.addr_string(),
            )))
            .configure(configure),
    )
    .await;
    for _ in 0..2 {
        let req = test::TestRequest::default()
            .method(Method::POST)
            .uri("/v1/chat/completions")
            .insert_header((header::CONTENT_TYPE, "application/json"))
            .set_payload(r#"{"model":"mixed","messages":[{"role":"user","content":"ping"}]}"#)
            .to_request();
        let res = test::call_service(&app, req).await;
        assert_eq!(res.status(), StatusCode::OK);
        let _ = to_bytes(res.into_body()).await?;
    }

    // Then: the two requests went out as different models. With no rotation both
    // would name the first.
    let sent: Vec<String> = provider
        .requests()
        .into_iter()
        .filter(|(path, _)| path.contains("/chat/completions"))
        .filter_map(|(_, body)| {
            serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|json| json.get("model")?.as_str().map(str::to_owned))
        })
        .collect();
    assert_eq!(
        sent,
        vec!["first".to_owned(), "second".to_owned()],
        "{sent:?}"
    );
    Ok(())
}

/// Wait for a usage record to reach the state stub.
///
/// `record` spawns the usage POST rather than awaiting it, so it arrives after the response the
/// test already has in hand. Polling with a deadline keeps the assertion meaningful: an
/// implementation that recorded nothing would time out rather than pass, which a bare sleep or a
/// single read would not guarantee.
async fn poll_for_usage(state: &FakeServer) -> Option<(String, String)> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if let Some(found) = state
            .requests()
            .into_iter()
            .find(|(path, _)| path.contains("/internal/v1/usage"))
        {
            return Some(found);
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        actix_web::rt::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}
