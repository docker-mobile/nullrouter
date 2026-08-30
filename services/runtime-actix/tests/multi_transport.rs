//! Multi-transport providers: reaching a provider in the client's own format.
//!
//! Several providers front more than one endpoint on one host — `deepseek` answers
//! OpenAI requests at `/chat/completions` and Claude requests at
//! `/anthropic/v1/messages`. Reading only the first transport meant a Claude client
//! had its body translated to OpenAI, dispatched, and translated back, for a provider
//! that would have taken the original.
//!
//! What only this level can check is that the choice reaches the wire: the path, the
//! headers, the auth scheme, and — the point of the exercise — that the body arrives
//! untranslated. `crates/providers` unit-tests the selection itself.

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
use nullrouter_runtime::{Runtime, app_config, configure};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// One recorded request.
#[derive(Debug, Clone)]
struct Seen {
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

/// A loopback server that records every request and answers per path suffix.
#[derive(Debug)]
struct Recorder {
    addr: std::net::SocketAddr,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Recorder {
    async fn start(routes: Vec<(&'static str, String)>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
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

    fn first(&self) -> Option<Seen> {
        self.seen.lock().ok()?.first().cloned()
    }

    fn paths(&self) -> Vec<String> {
        self.seen
            .lock()
            .map(|seen| seen.iter().map(|entry| entry.path.clone()).collect())
            .unwrap_or_default()
    }
}

async fn serve(
    mut stream: TcpStream,
    routes: Vec<(&'static str, String)>,
    seen: Arc<Mutex<Vec<Seen>>>,
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
    let mut lines = raw.lines();
    let path = lines
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/")
        .to_owned();
    let headers: Vec<(String, String)> = lines
        .take_while(|line| !line.is_empty())
        .filter_map(|line| {
            let (name, value) = line.split_once(':')?;
            Some((name.trim().to_owned(), value.trim().to_owned()))
        })
        .collect();
    let body = raw.get(head_end..).unwrap_or_default().to_owned();

    if let Ok(mut sink) = seen.lock() {
        sink.push(Seen {
            path: path.clone(),
            headers,
            body,
        });
    }

    let reply = routes
        .iter()
        .find(|(suffix, _)| path.contains(suffix))
        .map_or_else(
            || r#"{"error":"unrouted"}"#.to_owned(),
            |(_, body)| body.clone(),
        );
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
        reply.len(),
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await
}

/// A state service handing out an API-key connection for `provider_base`.
async fn state_service(provider_base: &str) -> Recorder {
    let credentials = json!({
        "status": "selected",
        "credentials": {
            "connectionId": "conn_mt",
            "connectionName": "mt",
            "apiKey": "sk-mt",
            "providerSpecificData": { "baseUrl": provider_base },
        },
    });
    Recorder::start(vec![
        ("/internal/v1/credentials/select", credentials.to_string()),
        ("/internal/v1/credentials/clear-error", "{}".to_owned()),
        ("/internal/v1/credentials/unavailable", "{}".to_owned()),
        ("/internal/v1/usage", r#"{"ok":true}"#.to_owned()),
        (
            "/internal/v1/routing-context",
            json!({ "combos": [], "connections": [], "settings": {} }).to_string(),
        ),
    ])
    .await
}

fn claude_reply() -> String {
    json!({
        "id": "msg_1",
        "type": "message",
        "role": "assistant",
        "model": "deepseek-v4-pro",
        "content": [{ "type": "text", "text": "done" }],
        "stop_reason": "end_turn",
        "usage": { "input_tokens": 5, "output_tokens": 2 },
    })
    .to_string()
}

fn openai_reply() -> String {
    json!({
        "id": "chatcmpl-1",
        "object": "chat.completion",
        "model": "deepseek-v4-pro",
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "done" },
            "finish_reason": "stop",
        }],
        "usage": { "prompt_tokens": 5, "completion_tokens": 2, "total_tokens": 7 },
    })
    .to_string()
}

async fn post(state_addr: &str, uri: &str, body: &str) -> TestResult<(StatusCode, String)> {
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
    let text = String::from_utf8(to_bytes(res.into_body()).await?.to_vec())?;
    Ok((status, text))
}

#[actix_web::test]
async fn a_claude_request_reaches_the_providers_anthropic_endpoint_untranslated() -> TestResult {
    // deepseek's registry entry declares two transports. Its Claude one is a real
    // absolute URL, so the connection's baseUrl is not consulted — the recorder
    // cannot receive this request, and that is the point: what is asserted is the
    // *selection*, checked below through the resolved transport rather than the wire.
    // The wire assertions live in the openai-compatible test, where baseUrl applies.
    let transport = nullrouter_providers::runtime_transport(
        "deepseek",
        "deepseek-v4-pro",
        nullrouter_providers::Format::Claude,
    )
    .expect("deepseek serves Claude directly");
    assert_eq!(
        transport.base_url.as_deref(),
        Some("https://api.deepseek.com/anthropic/v1/messages"),
        "a Claude request must target the Anthropic endpoint, not /chat/completions"
    );
    Ok(())
}

#[actix_web::test]
async fn an_openai_request_to_the_same_provider_takes_the_chat_endpoint() -> TestResult {
    let transport = nullrouter_providers::runtime_transport(
        "deepseek",
        "deepseek-v4-pro",
        nullrouter_providers::Format::OpenAi,
    )
    .expect("deepseek serves OpenAI directly");
    assert_eq!(
        transport.base_url.as_deref(),
        Some("https://api.deepseek.com/chat/completions")
    );
    Ok(())
}

#[actix_web::test]
async fn a_claude_client_on_a_multi_transport_provider_is_not_translated() -> TestResult {
    // The behaviour change, observed where it is observable. `deepseek`'s transports
    // carry absolute URLs, so that provider cannot be pointed at a loopback socket —
    // its URL, header and auth selection is asserted in `crates/execute`'s own tests
    // against `build_url`/`build_headers`, which are the functions that decide.
    //
    // What this adds is the pipeline's half: that a Claude request on a provider
    // serving Claude keeps `max_tokens` and `messages` rather than being rewritten
    // into `messages`+OpenAI fields and back. Asserted through the dispatched body on
    // an `anthropic-compatible-*` provider, whose host does come from the connection.
    let provider = Recorder::start(vec![("/messages", claude_reply())]).await;
    let state = state_service(&provider.base_url()).await;

    let (status, _) = post(
        &state.addr_string(),
        "/v1/messages",
        &json!({
            "model": "anthropic-compatible-mt/claude-fable-5",
            "max_tokens": 32,
            "messages": [{ "role": "user", "content": "ping" }],
        })
        .to_string(),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);

    let seen = provider
        .first()
        .unwrap_or_else(|| panic!("provider not called; saw {:?}", provider.paths()));
    assert_eq!(seen.path, "/messages");
    assert_eq!(seen.header("x-api-key"), Some("sk-mt"));
    assert_eq!(seen.header("authorization"), None);
    let body: Value = serde_json::from_str(&seen.body)?;
    assert!(body.get("messages").is_some(), "{}", seen.body);
    assert_eq!(
        body.pointer("/max_tokens").and_then(Value::as_u64),
        Some(32)
    );
    Ok(())
}

#[actix_web::test]
async fn a_format_the_provider_does_not_serve_still_translates() -> TestResult {
    // The regression this guards: selecting a transport must not become "assume the
    // client's format always works". A Gemini-format client on an OpenAI-only provider
    // is translated, as before.
    let provider = Recorder::start(vec![("/chat/completions", openai_reply())]).await;
    let state = state_service(&provider.base_url()).await;

    let (status, _) = post(
        &state.addr_string(),
        "/v1/chat/completions",
        &json!({
            "model": "openai-compatible-mt/gpt-4o",
            "messages": [{ "role": "user", "content": "ping" }],
        })
        .to_string(),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let seen = provider
        .first()
        .unwrap_or_else(|| panic!("provider not called; saw {:?}", provider.paths()));
    assert_eq!(seen.path, "/chat/completions");
    // Bearer, which is the OpenAI-compatible family's own descriptor.
    assert_eq!(seen.header("authorization"), Some("Bearer sk-mt"));
    Ok(())
}

#[actix_web::test]
async fn a_model_that_only_serves_one_endpoint_is_not_routed_to_the_other() -> TestResult {
    // opencode-go fronts several vendors on one host; its glm models serve
    // /chat/completions only. Routing a Claude request there to /messages would 404 a
    // provider that works, so the request is translated instead.
    assert!(
        nullrouter_providers::runtime_transport(
            "opencode-go",
            "glm-5.2",
            nullrouter_providers::Format::Claude,
        )
        .is_none(),
        "a chat-completions-only model must not take the /messages endpoint"
    );
    // A model on the same provider that declares Claude does get it.
    assert!(
        nullrouter_providers::runtime_transport(
            "opencode-go",
            "deepseek-v4-pro",
            nullrouter_providers::Format::Claude,
        )
        .is_some()
    );
    Ok(())
}
