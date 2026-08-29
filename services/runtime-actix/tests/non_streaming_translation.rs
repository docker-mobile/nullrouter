//! A non-streaming reply is translated back into the client's format.
//!
//! The streaming path translates every frame, and a provider that forces streaming
//! has its stream collapsed through the translator. But a provider that answers a
//! genuine non-streaming request in its own shape has no frames to translate — and
//! its body must still be converted, or an OpenAI client asking a Claude-format
//! provider for `stream: false` receives Claude's `content[]` where it expects
//! `choices[]`.

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

    /// A newline-delimited JSON reply, as Ollama sends.
    fn ndjson(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            content_type: "application/x-ndjson",
            body: body.into(),
        }
    }
}

#[derive(Debug)]
struct FakeServer {
    addr: std::net::SocketAddr,
    seen: Arc<Mutex<Vec<(String, String)>>>,
}

impl FakeServer {
    async fn start(routes: Vec<(&'static str, Reply)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("addr");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&seen);
        actix_web::rt::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let routes = routes.clone();
                let recorded = Arc::clone(&recorded);
                actix_web::rt::spawn(async move {
                    serve(stream, routes, recorded).await;
                });
            }
        });
        Self { addr, seen }
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    fn addr_string(&self) -> String {
        self.addr.to_string()
    }

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
) {
    let mut buffer = vec![0_u8; 65536];
    let mut filled = 0;
    let (head_end, content_length) = loop {
        let Ok(read) = stream
            .read(buffer.get_mut(filled..).unwrap_or_default())
            .await
        else {
            return;
        };
        if read == 0 {
            return;
        }
        filled += read;
        let text = String::from_utf8_lossy(buffer.get(..filled).unwrap_or_default()).into_owned();
        if let Some(index) = text.find("\r\n\r\n") {
            let length = text
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())?
                })
                .unwrap_or(0);
            break (index + 4, length);
        }
    };
    while filled < head_end + content_length {
        let Ok(read) = stream
            .read(buffer.get_mut(filled..).unwrap_or_default())
            .await
        else {
            break;
        };
        if read == 0 {
            break;
        }
        filled += read;
    }

    let raw = String::from_utf8_lossy(buffer.get(..filled).unwrap_or_default()).into_owned();
    let path = raw
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or_default()
        .to_owned();
    let body = raw.get(head_end..).unwrap_or_default().to_owned();
    if let Ok(mut sink) = seen.lock() {
        sink.push((path.clone(), body));
    }

    let reply = routes
        .iter()
        .find(|(suffix, _)| path.contains(suffix))
        .map_or_else(
            || Reply {
                status: 404,
                content_type: "application/json",
                body: String::from(r#"{"error":"not found"}"#),
            },
            |(_, reply)| reply.clone(),
        );
    let response = format!(
        "HTTP/1.1 {} OK\r\ncontent-type: {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        reply.status,
        reply.content_type,
        reply.body.len(),
        reply.body
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.flush().await;
}

async fn fake_state(provider_base: &str) -> FakeServer {
    let credentials = json!({
        "status": "selected",
        "credentials": {
            "connectionId": "conn_ns",
            "connectionName": "ns",
            "apiKey": "sk-ns",
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

async fn post(state_addr: &str, uri: &str, body: &str) -> TestResult<(StatusCode, Value)> {
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
    let raw = String::from_utf8(to_bytes(res.into_body()).await?.to_vec())?;
    let json = serde_json::from_str::<Value>(&raw).unwrap_or(Value::String(raw));
    Ok((status, json))
}

#[actix_rt::test]
async fn a_claude_providers_non_streaming_body_reaches_an_openai_client_as_openai() -> TestResult {
    // A Claude-format provider answering `stream: false` replies in Claude's own
    // shape: `content[]` blocks and `stop_reason`, with no `choices`.
    let provider = FakeServer::start(vec![(
        "/messages",
        Reply::json(
            json!({
                "id": "msg_ns",
                "type": "message",
                "role": "assistant",
                "model": "claude-sonnet-4-5",
                "content": [{ "type": "text", "text": "pong" }],
                "stop_reason": "end_turn",
                "usage": { "input_tokens": 3, "output_tokens": 1 },
            })
            .to_string(),
        ),
    )])
    .await;
    let state = fake_state(&provider.base_url()).await;

    let (status, body) = post(
        &state.addr_string(),
        "/v1/chat/completions",
        r#"{"model":"anthropic-compatible-ns/claude-sonnet-4-5","stream":false,"messages":[{"role":"user","content":"ping"}]}"#,
    )
    .await?;

    assert_eq!(status, StatusCode::OK, "{body}");
    // An OpenAI client reads `choices[0].message.content`. Returning Claude's own
    // body leaves that absent and the client sees an empty completion.
    assert_eq!(
        body.pointer("/choices/0/message/content"),
        Some(&json!("pong")),
        "the Claude body was not translated: {body}"
    );
    assert_eq!(
        body.get("object").and_then(Value::as_str),
        Some("chat.completion"),
        "{body}"
    );
    assert_eq!(
        body.pointer("/choices/0/finish_reason"),
        Some(&json!("stop")),
        "{body}"
    );
    // Claude's own field names must not survive into an OpenAI reply.
    assert!(body.get("content").is_none(), "{body}");
    assert!(body.get("stop_reason").is_none(), "{body}");
    // Usage is reported in OpenAI's spelling.
    assert_eq!(body.pointer("/usage/prompt_tokens"), Some(&json!(3)));
    assert_eq!(body.pointer("/usage/completion_tokens"), Some(&json!(1)));
    Ok(())
}

#[actix_rt::test]
async fn an_ollama_non_streaming_reply_is_translated_to_a_completion() -> TestResult {
    // Ollama answers `stream: false` with one JSON object carrying `message`.
    let provider = FakeServer::start(vec![(
        "/api/chat",
        Reply::json(
            json!({
                "model": "llama3.2",
                "message": { "role": "assistant", "content": "pong" },
                "done": true,
                "done_reason": "stop",
                "prompt_eval_count": 5,
                "eval_count": 2,
            })
            .to_string(),
        ),
    )])
    .await;
    let state = fake_state(&provider.base_url()).await;

    let (status, body) = post(
        &state.addr_string(),
        "/v1/chat/completions",
        r#"{"model":"ollama-local/llama3.2","stream":false,"messages":[{"role":"user","content":"ping"}]}"#,
    )
    .await?;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(
        body.pointer("/choices/0/message/content"),
        Some(&json!("pong")),
        "{body}"
    );
    assert_eq!(
        body.get("object").and_then(Value::as_str),
        Some("chat.completion"),
        "{body}"
    );
    // Ollama's own counters become OpenAI's token fields.
    assert_eq!(body.pointer("/usage/prompt_tokens"), Some(&json!(5)));
    assert_eq!(body.pointer("/usage/completion_tokens"), Some(&json!(2)));
    assert_eq!(body.pointer("/usage/total_tokens"), Some(&json!(7)));
    // `message`/`done` are Ollama's shape and must not reach the client.
    assert!(body.get("done").is_none(), "{body}");

    // And the request that went out was in Ollama's shape.
    let requests = provider.requests();
    let (path, sent) = requests.first().expect("provider was called");
    assert!(path.contains("/api/chat"), "got {path}");
    let sent: Value = serde_json::from_str(sent)?;
    assert_eq!(sent.get("model"), Some(&json!("llama3.2")));
    assert_eq!(
        sent.pointer("/messages/0/content"),
        Some(&json!("ping")),
        "{sent}"
    );
    Ok(())
}

#[actix_rt::test]
async fn an_ollama_stream_is_translated_frame_by_frame() -> TestResult {
    // Ollama streams NDJSON, not SSE: each line is a whole JSON object.
    let stream = [
        r#"{"model":"llama3.2","message":{"role":"assistant","content":"po"}}"#,
        r#"{"model":"llama3.2","message":{"role":"assistant","content":"ng"}}"#,
        r#"{"model":"llama3.2","done":true,"done_reason":"stop","prompt_eval_count":5,"eval_count":2}"#,
    ]
    .join("\n");
    let provider =
        FakeServer::start(vec![("/api/chat", Reply::ndjson(format!("{stream}\n")))]).await;
    let state = fake_state(&provider.base_url()).await;

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(Runtime::with_state_addr(
                &state.addr_string(),
            )))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::default()
        .method(Method::POST)
        .uri("/v1/chat/completions")
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(
            r#"{"model":"ollama-local/llama3.2","stream":true,"messages":[{"role":"user","content":"ping"}]}"#
                .to_owned(),
        )
        .to_request();
    let res = test::call_service(&app, req).await;
    assert_eq!(res.status(), StatusCode::OK);
    let raw = String::from_utf8(to_bytes(res.into_body()).await?.to_vec())?;

    // The client gets OpenAI SSE, whatever framing the provider used.
    let payloads: Vec<Value> = raw
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter(|payload| *payload != "[DONE]")
        .filter_map(|payload| serde_json::from_str(payload).ok())
        .collect();
    let text: String = payloads
        .iter()
        .filter_map(|chunk| {
            chunk
                .pointer("/choices/0/delta/content")
                .and_then(Value::as_str)
        })
        .collect();
    assert_eq!(text, "pong", "got frames: {raw}");
    assert!(raw.contains("data: [DONE]"), "{raw}");
    // The terminal chunk reports the finish reason and usage.
    let finished = payloads
        .iter()
        .any(|chunk| chunk.pointer("/choices/0/finish_reason") == Some(&json!("stop")));
    assert!(finished, "no finish_reason in {raw}");
    Ok(())
}
