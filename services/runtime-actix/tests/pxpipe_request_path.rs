//! PXPIPE on the request path, end to end.
//!
//! A real state service says the saver is on, a real `node` worker transforms the
//! body, and a real provider socket records what arrived. That last part is the point:
//! every other test in this feature could pass while the transformed body was
//! computed, logged, and then dropped on the floor. The only proof that the token
//! saver saves anything is the bytes the provider received.
//!
//! These need `node` on the path and fail rather than skip without it, for the same
//! reason as `nullrouter-pxpipe`'s own worker tests: a suite that quietly passed
//! would report the feature as covered when nothing ran.

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
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::{TcpListener, TcpStream};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A loopback server that answers every request with one reply and records what it
/// was sent.
#[derive(Debug)]
struct Recorder {
    addr: std::net::SocketAddr,
    seen: Arc<Mutex<Vec<(String, String)>>>,
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

    /// The body sent to the first path containing `suffix`.
    fn body_for(&self, suffix: &str) -> Option<String> {
        self.seen
            .lock()
            .ok()?
            .iter()
            .find_map(|(path, body)| path.contains(suffix).then(|| body.clone()))
    }
}

async fn serve(
    mut stream: TcpStream,
    routes: Vec<(&'static str, String)>,
    seen: Arc<Mutex<Vec<(String, String)>>>,
) -> std::io::Result<()> {
    let mut buffer = Vec::new();
    let mut chunk = vec![0_u8; 65_536];

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

/// A state service handing out credentials for `provider_base`, with the PXPIPE
/// settings the test wants.
async fn state_service(provider_base: &str, settings: Value) -> Recorder {
    let credentials = json!({
        "status": "selected",
        "credentials": {
            "connectionId": "conn_px",
            "connectionName": "px",
            "apiKey": "sk-px",
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
            json!({ "combos": [], "connections": [], "settings": settings }).to_string(),
        ),
    ])
    .await
}

/// A provider that answers a minimal Claude message.
fn claude_reply() -> String {
    json!({
        "id": "msg_1",
        "type": "message",
        "role": "assistant",
        "model": "claude-fable-5",
        "content": [{ "type": "text", "text": "done" }],
        "stop_reason": "end_turn",
        "usage": { "input_tokens": 10, "output_tokens": 2 },
    })
    .to_string()
}

/// Write a stub `pxpipe-proxy` under `data_dir`.
fn install_stub(data_dir: &std::path::Path, source: &str) {
    let core = data_dir
        .join("pxpipe")
        .join("node_modules")
        .join("pxpipe-proxy")
        .join("dist")
        .join("core");
    std::fs::create_dir_all(&core).expect("create package tree");
    std::fs::write(
        core.parent()
            .and_then(std::path::Path::parent)
            .map(|root| root.join("package.json"))
            .expect("package root"),
        "{\"name\":\"pxpipe-proxy\",\"version\":\"1.0.0\",\"type\":\"module\"}",
    )
    .expect("write manifest");
    std::fs::write(core.join("library.js"), source).expect("write library");
}

/// A transform that marks the body so its arrival at the provider is unmistakable.
const MARKING: &str = r#"
export async function transformAnthropicMessages({ body }) {
  const request = JSON.parse(new TextDecoder().decode(body));
  request.messages = [{
    role: "user",
    content: [
      { type: "image", source: { type: "base64", media_type: "image/png", data: "iVBORw0KGgo=" } },
      { type: "text", text: "PXPIPE_IMAGED" },
    ],
  }];
  return {
    applied: true,
    reason: "applied",
    body: new TextEncoder().encode(JSON.stringify(request)),
    info: { compressedChars: 30000, outgoingTextChars: 40, imageCount: 1, imagePixels: 750000, baselineTokens: 8000 },
    cache: { ownsCacheControl: false },
  };
}
"#;

fn require_node() {
    assert!(
        nullrouter_pxpipe::install::find_node().is_some(),
        "these tests exercise the Node transform worker and need `node` on the PATH"
    );
}

/// Settings with the saver on.
fn enabled_settings() -> Value {
    json!({
        "pxpipeEnabled": true,
        "pxpipeAutoInstall": false,
        "pxpipeMinChars": 1000,
        "pxpipeTimeoutMs": 15000,
    })
}

/// A Claude request large enough to clear the gate.
fn large_claude_request() -> String {
    json!({
        "model": "anthropic-compatible-px/claude-fable-5",
        "max_tokens": 64,
        "messages": [{
            "role": "user",
            "content": [{ "type": "text", "text": "context ".repeat(400) }],
        }],
    })
    .to_string()
}

async fn post(
    state_addr: &str,
    data_dir: &std::path::Path,
    uri: &str,
    body: &str,
) -> TestResult<(StatusCode, String)> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(Runtime::with_state_addr_and_pxpipe_dir(
                state_addr, data_dir,
            )))
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
    let body = String::from_utf8(to_bytes(res.into_body()).await?.to_vec())?;
    Ok((status, body))
}

/// The recorded event log under `data_dir`.
fn events(data_dir: &std::path::Path) -> Vec<Value> {
    let path = data_dir.join("pxpipe").join("events.jsonl");
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect()
}

#[actix_web::test]
async fn the_compressed_body_is_what_the_provider_receives() -> TestResult {
    require_node();
    let dir = tempfile::tempdir()?;
    install_stub(dir.path(), MARKING);
    let provider = Recorder::start(vec![("/messages", claude_reply())]).await;
    let state = state_service(&provider.base_url(), enabled_settings()).await;

    let (status, _body) = post(
        &state.addr_string(),
        dir.path(),
        "/v1/messages",
        &large_claude_request(),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);

    // The only assertion that proves the feature does anything: the transform's own
    // output is on the wire. Everything else — the summary, the log line, the stats —
    // could be right while the original body was dispatched.
    let sent = provider
        .body_for("/messages")
        .expect("the provider was called");
    assert!(
        sent.contains("PXPIPE_IMAGED"),
        "the provider received the untransformed body: {}",
        sent.chars().take(400).collect::<String>()
    );
    assert!(
        sent.contains("\"type\":\"image\""),
        "the images are on the wire"
    );
    // And the bulk is gone, which is the whole purpose.
    assert!(
        !sent.contains("context context context"),
        "the original context was still sent"
    );

    let recorded = events(dir.path());
    assert_eq!(recorded.len(), 1, "one attempt, one event");
    let event = recorded.first().expect("the event");
    assert_eq!(event.pointer("/applied"), Some(&Value::Bool(true)));
    assert_eq!(
        event.pointer("/reason").and_then(Value::as_str),
        Some("applied")
    );
    // 40 chars of remaining text → 10 tokens, plus 750 000 px / 750 → 1 000.
    assert_eq!(
        event.pointer("/tokensAfterEst").and_then(Value::as_u64),
        Some(1_010)
    );
    assert_eq!(
        event.pointer("/tokensSavedEst").and_then(Value::as_u64),
        Some(8_000 - 1_010)
    );
    Ok(())
}

#[actix_web::test]
async fn a_disabled_saver_sends_the_original_body_and_records_nothing() -> TestResult {
    require_node();
    let dir = tempfile::tempdir()?;
    install_stub(dir.path(), MARKING);
    let provider = Recorder::start(vec![("/messages", claude_reply())]).await;
    // Installed and working, but the setting is off. The transform must not run —
    // being installed is not consent.
    let state = state_service(
        &provider.base_url(),
        json!({ "pxpipeEnabled": false, "pxpipeMinChars": 1000 }),
    )
    .await;

    let (status, _) = post(
        &state.addr_string(),
        dir.path(),
        "/v1/messages",
        &large_claude_request(),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let sent = provider
        .body_for("/messages")
        .expect("the provider was called");
    assert!(
        !sent.contains("PXPIPE_IMAGED"),
        "a disabled saver transformed a request"
    );
    assert!(
        sent.contains("context context"),
        "the original body was sent"
    );
    // Not even a skip event: an off switch means the feature is not running at all,
    // and writing a line per request would be a log that grows for nothing.
    assert!(
        events(dir.path()).is_empty(),
        "a disabled saver wrote to the event log"
    );
    Ok(())
}

#[actix_web::test]
async fn a_body_below_the_threshold_is_dispatched_unchanged_and_the_skip_is_recorded() -> TestResult
{
    require_node();
    let dir = tempfile::tempdir()?;
    install_stub(dir.path(), MARKING);
    let provider = Recorder::start(vec![("/messages", claude_reply())]).await;
    let state = state_service(&provider.base_url(), enabled_settings()).await;

    let small = json!({
        "model": "anthropic-compatible-px/claude-fable-5",
        "max_tokens": 16,
        "messages": [{ "role": "user", "content": "hi" }],
    })
    .to_string();
    let (status, _) = post(&state.addr_string(), dir.path(), "/v1/messages", &small).await?;
    assert_eq!(status, StatusCode::OK);
    let sent = provider
        .body_for("/messages")
        .expect("the provider was called");
    assert!(!sent.contains("PXPIPE_IMAGED"));

    // Recorded, unlike the disabled case: the saver *is* running, and "why did nothing
    // happen to my request" is answered by this line.
    let recorded = events(dir.path());
    assert_eq!(recorded.len(), 1);
    assert_eq!(
        recorded
            .first()
            .and_then(|event| event.pointer("/reason"))
            .and_then(Value::as_str),
        Some("below_threshold")
    );
    Ok(())
}

#[actix_web::test]
async fn a_non_claude_target_is_refused_rather_than_mangled() -> TestResult {
    require_node();
    let dir = tempfile::tempdir()?;
    install_stub(dir.path(), MARKING);
    let provider = Recorder::start(vec![("/chat/completions", json!({
        "id": "chatcmpl-1",
        "object": "chat.completion",
        "model": "gpt-4o",
        "choices": [{ "index": 0, "message": { "role": "assistant", "content": "done" }, "finish_reason": "stop" }],
        "usage": { "prompt_tokens": 10, "completion_tokens": 2, "total_tokens": 12 },
    }).to_string())]).await;
    let state = state_service(&provider.base_url(), enabled_settings()).await;

    // An OpenAI-format target. The package rewrites Anthropic content blocks, so
    // handing it an OpenAI body would corrupt the request rather than compress it.
    let body = json!({
        "model": "openai-compatible-px/gpt-4o",
        "messages": [{ "role": "user", "content": "context ".repeat(400) }],
    })
    .to_string();
    let (status, _) = post(
        &state.addr_string(),
        dir.path(),
        "/v1/chat/completions",
        &body,
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let sent = provider
        .body_for("/chat/completions")
        .expect("the provider was called");
    assert!(!sent.contains("PXPIPE_IMAGED"), "an OpenAI body was imaged");
    assert!(sent.contains("context context"));

    let recorded = events(dir.path());
    assert_eq!(
        recorded
            .first()
            .and_then(|event| event.pointer("/reason"))
            .and_then(Value::as_str),
        Some("unsupported_format")
    );
    assert_eq!(
        recorded
            .first()
            .and_then(|event| event.pointer("/detail"))
            .and_then(Value::as_str),
        Some("openai"),
        "the format that was refused is named"
    );
    Ok(())
}

#[actix_web::test]
async fn a_transform_that_returns_unparseable_json_cannot_break_the_request() -> TestResult {
    require_node();
    let dir = tempfile::tempdir()?;
    // A transform that claims success and hands back bytes that are not JSON. Nothing
    // stops a future package version, or a corrupted install, from doing this.
    install_stub(
        dir.path(),
        r#"
export async function transformAnthropicMessages() {
  return {
    applied: true,
    reason: "applied",
    body: new TextEncoder().encode("this is not json at all"),
    info: { imageCount: 1 },
  };
}
"#,
    );
    let provider = Recorder::start(vec![("/messages", claude_reply())]).await;
    let state = state_service(&provider.base_url(), enabled_settings()).await;

    let (status, _) = post(
        &state.addr_string(),
        dir.path(),
        "/v1/messages",
        &large_claude_request(),
    )
    .await?;
    // The request succeeds, with the original body: a token saver must never be able
    // to turn a valid request into a broken one.
    assert_eq!(status, StatusCode::OK);
    let sent = provider
        .body_for("/messages")
        .expect("the provider was called");
    assert!(
        sent.contains("context context"),
        "the original body was dispatched"
    );
    assert!(
        serde_json::from_str::<Value>(&sent).is_ok(),
        "the provider received valid JSON"
    );
    Ok(())
}

#[actix_web::test]
async fn a_timed_out_transform_dispatches_the_original_and_the_request_still_succeeds() -> TestResult
{
    require_node();
    let dir = tempfile::tempdir()?;
    install_stub(
        dir.path(),
        r#"
export async function transformAnthropicMessages() {
  const until = Date.now() + 30000;
  while (Date.now() < until) { /* uninterruptible */ }
}
"#,
    );
    let provider = Recorder::start(vec![("/messages", claude_reply())]).await;
    let state = state_service(
        &provider.base_url(),
        json!({
            "pxpipeEnabled": true,
            "pxpipeMinChars": 1000,
            // A short budget, so the test does not wait 30 seconds for the answer the
            // hung worker will never give.
            "pxpipeTimeoutMs": 300,
        }),
    )
    .await;

    let started = std::time::Instant::now();
    let (status, _) = post(
        &state.addr_string(),
        dir.path(),
        "/v1/messages",
        &large_claude_request(),
    )
    .await?;
    let elapsed = started.elapsed();
    assert_eq!(
        status,
        StatusCode::OK,
        "a hung transform must not fail the request"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "the request waited for the hung transform: {elapsed:?}"
    );
    let sent = provider
        .body_for("/messages")
        .expect("the provider was called");
    assert!(sent.contains("context context"));

    let recorded = events(dir.path());
    assert_eq!(
        recorded
            .first()
            .and_then(|event| event.pointer("/reason"))
            .and_then(Value::as_str),
        Some("timeout")
    );
    Ok(())
}

#[actix_web::test]
async fn an_enabled_saver_with_nothing_installed_still_serves_the_request() -> TestResult {
    let dir = tempfile::tempdir()?;
    // Deliberately no install. This is the state a user is in the moment they turn the
    // setting on, and it must not cost them a single request.
    let provider = Recorder::start(vec![("/messages", claude_reply())]).await;
    let state = state_service(&provider.base_url(), enabled_settings()).await;

    let (status, _) = post(
        &state.addr_string(),
        dir.path(),
        "/v1/messages",
        &large_claude_request(),
    )
    .await?;
    assert_eq!(status, StatusCode::OK);
    let sent = provider
        .body_for("/messages")
        .expect("the provider was called");
    assert!(sent.contains("context context"));
    let recorded = events(dir.path());
    assert_eq!(
        recorded
            .first()
            .and_then(|event| event.pointer("/reason"))
            .and_then(Value::as_str),
        Some("not_installed"),
        "the reason a user can act on is recorded: {recorded:?}"
    );
    Ok(())
}
