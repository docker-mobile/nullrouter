//! The translator inspector runs the real engine.
//!
//! The dashboard's inspector used to echo shapes back: `sourceFormat: "unknown"` and an empty
//! body. It now runs the same translation the live `/v1` path runs.
//!
//! **The assertion that matters is agreement with `crates/translate`.** An inspector is a
//! debugging tool, and one that shows a translation nobody performs is worse than none at all
//! — a user would chase a discrepancy that exists only in the inspector. So the tests here
//! compute the expected value by calling the engine directly and compare, rather than pinning
//! a literal that could drift away from the engine without anyone noticing.
//!
//! Also covered: credentials never appear in the headers pane. Those panes get screenshotted
//! into bug reports.

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
    http::{Method, StatusCode, header},
    test, web,
};
use nullrouter_providers::Format;
use nullrouter_runtime::{Runtime, app_config, configure};
use nullrouter_translate::{RequestRoute, state::Clock, state::StreamState};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const NODE_ID: &str = "anthropic-compatible-99999999-8888-7777-6666-555555555555";
const PREFIX: &str = "myclaude";

/// A loopback state service answering the routes the inspector needs.
#[derive(Debug)]
struct FakeState {
    addr: std::net::SocketAddr,
}

impl FakeState {
    async fn start(routes: Vec<(&'static str, String)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("addr");
        let routes = Arc::new(Mutex::new(routes));
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    break;
                };
                let routes = Arc::clone(&routes);
                tokio::spawn(async move {
                    let _ = serve(stream, routes).await;
                });
            }
        });
        Self { addr }
    }

    fn addr_string(&self) -> String {
        self.addr.to_string()
    }
}

async fn serve(
    mut stream: TcpStream,
    routes: Arc<Mutex<Vec<(&'static str, String)>>>,
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
    let path = raw
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/")
        .to_owned();

    let reply = routes
        .lock()
        .ok()
        .and_then(|routes| {
            routes
                .iter()
                .find(|(suffix, _)| path.contains(suffix))
                .map(|(_, body)| body.clone())
        })
        .unwrap_or_else(|| r#"{"error":"unrouted"}"#.to_owned());

    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
        reply.len(),
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await
}

/// A state service whose one connection is a Claude-format compatible node.
async fn state_service() -> FakeState {
    let credentials = json!({
        "status": "selected",
        "credentials": {
            "connectionId": "conn_inspect",
            "connectionName": "myclaude",
            "apiKey": "sk-super-secret-key-value",
            "providerSpecificData": {
                "baseUrl": "https://provider.example/v1",
                "prefix": PREFIX,
            },
        },
    });
    let routing = json!({
        "combos": [],
        "connections": [{ "provider": NODE_ID, "prefix": PREFIX, "enabledModels": [] }],
        "settings": {},
    });
    FakeState::start(vec![
        ("/internal/v1/credentials/select", credentials.to_string()),
        ("/internal/v1/routing-context", routing.to_string()),
        ("/internal/v1/probe-targets", r#"{"targets":[]}"#.to_owned()),
    ])
    .await
}

async fn step(state_addr: &str, payload: Value) -> TestResult<(StatusCode, Value)> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(Runtime::with_state_addr(state_addr)))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::default()
        .method(Method::POST)
        .uri("/internal/translator/step")
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(payload.to_string())
        .to_request();
    let res = test::call_service(&app, req).await;
    let status = res.status();
    let body = String::from_utf8(to_bytes(res.into_body()).await?.to_vec())?;
    Ok((status, serde_json::from_str(&body)?))
}

fn claude_client_body() -> Value {
    json!({
        "model": format!("{PREFIX}/some-claude-model"),
        "max_tokens": 256,
        "system": "Be brief.",
        "messages": [{ "role": "user", "content": "Hello" }],
    })
}

#[actix_web::test]
async fn step_one_reports_the_real_formats_not_unknown() -> TestResult {
    let state = state_service().await;
    let (status, body) = step(
        &state.addr_string(),
        json!({ "step": 1, "body": claude_client_body() }),
    )
    .await?;

    assert_eq!(status, StatusCode::OK, "{body}");
    let result = &body["result"];
    // The prefix resolves to the node id, exactly as the live path resolves it.
    assert_eq!(result["provider"], NODE_ID, "{result}");
    assert_eq!(result["model"], "some-claude-model");
    // A Claude body posted to a Claude-format provider: both ends claude, no translation.
    assert_eq!(result["sourceFormat"], "claude", "{result}");
    assert_eq!(result["targetFormat"], "claude", "{result}");
    // The bug this replaces.
    assert_ne!(result["sourceFormat"], "unknown");
    Ok(())
}

#[actix_web::test]
async fn step_two_matches_what_the_engine_produces() -> TestResult {
    let state = state_service().await;
    let client_body = claude_client_body();
    let (status, body) = step(
        &state.addr_string(),
        json!({ "step": 2, "body": client_body.clone() }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Computed rather than pinned: a literal here could drift from the engine and the
    // inspector would keep agreeing with the literal.
    let expected = nullrouter_translate::translate_request(
        RequestRoute {
            source: Format::Claude,
            target: Format::OpenAi,
            provider: NODE_ID,
            model: "some-claude-model",
        },
        &client_body,
        true,
        0,
    );

    assert_eq!(
        body["result"]["body"], expected.body,
        "the inspector disagreed with crates/translate"
    );
    // And it is a real translation, not an echo: Claude's `system` becomes an OpenAI system
    // message, so the shape must actually have changed.
    assert!(
        body["result"]["body"]["messages"].is_array(),
        "{}",
        body["result"]["body"]
    );
    Ok(())
}

#[actix_web::test]
async fn step_three_matches_the_engine_and_reports_the_wire() -> TestResult {
    let state = state_service().await;
    let openai_body = json!({
        "model": "some-claude-model",
        "max_tokens": 256,
        "messages": [
            { "role": "system", "content": "Be brief." },
            { "role": "user", "content": "Hello" },
        ],
    });
    let (status, body) = step(
        &state.addr_string(),
        json!({
            "step": 3,
            "body": openai_body.clone(),
            "provider": NODE_ID,
            "model": "some-claude-model",
        }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{body}");

    let expected = nullrouter_translate::translate_request(
        RequestRoute {
            source: Format::OpenAi,
            target: Format::Claude,
            provider: NODE_ID,
            model: "some-claude-model",
        },
        &openai_body,
        true,
        0,
    );
    assert_eq!(
        body["result"]["body"], expected.body,
        "the inspector disagreed with crates/translate"
    );

    // The wire panes: the URL the request would go to, from the connection.
    let url = body["result"]["url"].as_str().unwrap_or_default();
    assert!(
        url.starts_with("https://provider.example/v1"),
        "unexpected url {url:?}"
    );
    assert!(
        body["result"]["headers"].is_object(),
        "headers pane missing: {}",
        body["result"]
    );
    Ok(())
}

#[actix_web::test]
async fn the_headers_pane_never_shows_a_credential() -> TestResult {
    // These panes end up in screenshots and bug reports. The *scheme* is what a user needs to
    // debug auth; the key is not.
    let state = state_service().await;
    let (_, body) = step(
        &state.addr_string(),
        json!({
            "step": 3,
            "body": { "model": "m", "messages": [] },
            "provider": NODE_ID,
            "model": "some-claude-model",
        }),
    )
    .await?;

    let rendered = body.to_string();
    assert!(
        !rendered.contains("sk-super-secret-key-value"),
        "the API key leaked into the inspector: {rendered}"
    );
    let headers = body["result"]["headers"]
        .as_object()
        .expect("a headers object");
    assert!(!headers.is_empty(), "no headers were reported");
    // Whichever header carries the key for this provider, it must be redacted and must say so.
    let auth_like: Vec<&String> = headers
        .keys()
        .filter(|name| {
            let lower = name.to_ascii_lowercase();
            lower == "authorization" || lower.contains("api-key")
        })
        .collect();
    assert!(
        !auth_like.is_empty(),
        "expected an auth header among {:?}",
        headers.keys().collect::<Vec<_>>()
    );
    for name in auth_like {
        let value = headers[name].as_str().unwrap_or_default();
        assert!(
            value.contains("redacted"),
            "{name} was not redacted: {value:?}"
        );
    }
    Ok(())
}

#[actix_web::test]
async fn step_five_translates_a_response_and_matches_the_engine() -> TestResult {
    // This port's addition. Upstream leaves its response panes to be pasted into by hand.
    let state = state_service().await;
    let chunks = vec![
        json!({"type": "message_start", "message": {"id": "msg_1", "model": "m",
               "role": "assistant", "content": [], "usage": {"input_tokens": 5, "output_tokens": 0}}}),
        json!({"type": "content_block_start", "index": 0,
               "content_block": {"type": "text", "text": ""}}),
        json!({"type": "content_block_delta", "index": 0,
               "delta": {"type": "text_delta", "text": "Hi"}}),
        json!({"type": "message_stop"}),
    ];

    let (status, body) = step(
        &state.addr_string(),
        json!({
            "step": 5,
            "provider": NODE_ID,
            "body": { "sourceFormat": "openai" },
            "chunks": chunks.clone(),
        }),
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{body}");

    // Claude provider -> OpenAI client, threading one state across chunks the way the live
    // stream does. Translating chunks independently would lose the cross-chunk assembly.
    let mut expected_state = StreamState::new(Clock::System);
    let mut expected: Vec<Value> = Vec::new();
    for chunk in &chunks {
        expected.extend(nullrouter_translate::translate_response(
            Format::Claude,
            Format::OpenAi,
            chunk,
            &mut expected_state,
        ));
    }

    let client = body["result"]["client"]
        .as_array()
        .expect("a client array")
        .clone();
    assert_eq!(
        client.len(),
        expected.len(),
        "chunk count differs: {client:?} vs {expected:?}"
    );
    // Compare the text content rather than whole objects: both sides stamp `created` from the
    // system clock, so identical translations can differ by a second.
    let text_of = |chunks: &[Value]| -> String {
        chunks
            .iter()
            .filter_map(|chunk| {
                chunk
                    .pointer("/choices/0/delta/content")
                    .and_then(Value::as_str)
            })
            .collect()
    };
    assert_eq!(text_of(&client), text_of(&expected));
    assert_eq!(text_of(&client), "Hi", "the translation lost the content");

    assert_eq!(body["result"]["targetFormat"], "claude");
    assert_eq!(body["result"]["sourceFormat"], "openai");
    assert!(
        body["result"]["openai"].is_array(),
        "the intermediate pane is missing"
    );
    Ok(())
}

#[actix_web::test]
async fn an_invalid_step_is_refused() -> TestResult {
    let state = state_service().await;
    for step_number in [0, 4, 6, 99] {
        let (status, _) = step(
            &state.addr_string(),
            json!({ "step": step_number, "body": {"model": "m"} }),
        )
        .await?;
        assert_eq!(
            status,
            StatusCode::BAD_REQUEST,
            "step {step_number} should be refused"
        );
    }
    Ok(())
}

#[actix_web::test]
async fn a_missing_body_is_refused() -> TestResult {
    let state = state_service().await;
    let (status, _) = step(&state.addr_string(), json!({ "step": 1 })).await?;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    Ok(())
}

#[actix_web::test]
async fn a_model_that_resolves_to_nothing_is_reported() -> TestResult {
    // A bare alias with no matching combo or connection. The inspector should say so rather
    // than answer with an empty translation that looks like a result.
    let state = state_service().await;
    let (status, body) = step(
        &state.addr_string(),
        json!({ "step": 1, "body": {"model": ""} }),
    )
    .await?;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    Ok(())
}
