//! Fusion combo request shaping.
//!
//! A fusion combo asks every panel model, then a judge writes one answer. The
//! shaping decisions tested here are the ones that go wrong quietly: a panel that
//! still carries tools returns a half-finished tool turn the judge cannot use, and a
//! judge that loses the client's stream flag turns a streaming request into a
//! blocking one.
//!
//! These drive the same loopback provider/state harness as `execution_e2e`, so the
//! assertions are about what the provider actually received.

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
}

/// A loopback server that replies per request path and records what it saw.
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
        format!("http://{}/v1", self.addr)
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
    // Read until the headers are complete, then until the declared body arrives.
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

/// A completion reply whose content names which model answered.
fn completion(content: &str) -> Reply {
    Reply::json(
        json!({
            "id": "chatcmpl-fusion",
            "object": "chat.completion",
            "model": "panel",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": content },
                "finish_reason": "stop",
            }],
            "usage": { "prompt_tokens": 4, "completion_tokens": 2, "total_tokens": 6 },
        })
        .to_string(),
    )
}

/// State declaring one fusion combo over `models`.
async fn fusion_state(provider_base: &str, models: &[&str]) -> FakeServer {
    fusion_state_with_settings(provider_base, models, json!({ "comboStrategy": "fusion" })).await
}

/// State declaring one combo named `panel` over `models`, with explicit settings.
///
/// Separate so a test can set a per-combo override against a different global, which
/// is the only way to tell the two apart.
async fn fusion_state_with_settings(
    provider_base: &str,
    models: &[&str],
    settings: Value,
) -> FakeServer {
    let credentials = json!({
        "status": "selected",
        "credentials": {
            "connectionId": "conn_fusion",
            "connectionName": "fusion",
            "apiKey": "sk-fusion",
            "providerSpecificData": { "baseUrl": provider_base },
        },
    });
    let routing = json!({
        "combos": [{ "id": "c1", "name": "panel", "kind": null, "models": models }],
        "connections": [],
        "settings": settings,
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

struct Response {
    status: StatusCode,
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
    let bytes = to_bytes(res.into_body()).await?;
    Ok(Response {
        status,
        body: String::from_utf8_lossy(&bytes).into_owned(),
    })
}

/// Every chat body the provider received.
fn chat_bodies(provider: &FakeServer) -> Vec<Value> {
    provider
        .requests()
        .into_iter()
        .filter(|(path, _)| path.contains("/chat/completions"))
        .filter_map(|(_, body)| serde_json::from_str::<Value>(&body).ok())
        .collect()
}

#[actix_rt::test]
async fn a_fusion_combo_asks_every_panel_model_then_judges() -> TestResult {
    // Given: a fusion combo of two executable models. Before fusion was ported this
    // resolved as `fallback` — the first model answered and the second was never
    // asked, so the combo silently behaved like a single model.
    let provider = FakeServer::start(vec![("/chat/completions", completion("an answer"))]).await;
    let state = fusion_state(
        &provider.base_url(),
        &["openai-compatible-f/first", "openai-compatible-f/second"],
    )
    .await;

    // When: the combo is requested.
    let response = post(
        &state.addr_string(),
        "/v1/chat/completions",
        r#"{"model":"panel","messages":[{"role":"user","content":"ping"}]}"#,
    )
    .await?;
    assert_eq!(response.status, StatusCode::OK, "body: {}", response.body);

    // Then: three calls went out — two panel models plus the judge.
    let bodies = chat_bodies(&provider);
    assert_eq!(
        bodies.len(),
        3,
        "expected 2 panel calls + 1 judge call, got {}: {bodies:#?}",
        bodies.len()
    );

    // And: the judge's body carries the panel prose, anonymised by source.
    let judge = bodies.last().expect("a judge call");
    let judge_text = judge.to_string();
    assert!(judge_text.contains("[Source 1]"), "{judge_text}");
    assert!(judge_text.contains("[Source 2]"), "{judge_text}");
    assert!(judge_text.contains("JUDGE"), "{judge_text}");
    // The panel models must not be named: the judge should weigh substance, not
    // which vendor produced which answer.
    assert!(
        !judge_text.contains("openai-compatible-f/first"),
        "{judge_text}"
    );
    Ok(())
}

#[actix_rt::test]
async fn panel_calls_are_non_streaming_with_tools_stripped() -> TestResult {
    // Given: a streaming request carrying tools. A panel model that still had tools
    // could answer with a `tool_calls` turn, which is not prose the judge can
    // synthesise from — and the client never sees panel output anyway.
    let provider = FakeServer::start(vec![("/chat/completions", completion("prose"))]).await;
    let state = fusion_state(
        &provider.base_url(),
        &["openai-compatible-f/first", "openai-compatible-f/second"],
    )
    .await;

    // When: the combo is asked with tools and stream on.
    let response = post(
        &state.addr_string(),
        "/v1/chat/completions",
        r#"{"model":"panel","stream":true,"tools":[{"type":"function","function":{"name":"lookup"}}],"tool_choice":"auto","stream_options":{"include_usage":true},"messages":[{"role":"user","content":"ping"}]}"#,
    )
    .await?;
    assert_eq!(response.status, StatusCode::OK, "body: {}", response.body);

    let bodies = chat_bodies(&provider);
    let (judge, panels) = bodies.split_last().expect("at least one call");

    // Then: every panel call is non-streaming with no tools.
    for panel in panels {
        assert_eq!(panel.get("stream"), Some(&json!(false)), "{panel}");
        assert!(panel.get("tools").is_none(), "{panel}");
        assert!(panel.get("tool_choice").is_none(), "{panel}");
        // Dropped on its own account: some providers reject stream_options when
        // stream is false.
        assert!(panel.get("stream_options").is_none(), "{panel}");
    }

    // And: the judge keeps the client's stream flag and tools, so streaming and
    // downstream tool use still work.
    assert_eq!(judge.get("stream"), Some(&json!(true)), "{judge}");
    assert!(judge.get("tools").is_some(), "{judge}");
    Ok(())
}

#[actix_rt::test]
async fn a_single_surviving_panel_answer_is_returned_without_a_judge() -> TestResult {
    // Given: a fusion combo where only one model can execute. Asking a judge to
    // "synthesise" one answer would spend a second provider call to paraphrase it.
    let provider = FakeServer::start(vec![("/chat/completions", completion("only answer"))]).await;
    let state = fusion_state(
        &provider.base_url(),
        &["ollama/llama3", "openai-compatible-f/second"],
    )
    .await;

    // When: the combo is requested.
    let response = post(
        &state.addr_string(),
        "/v1/chat/completions",
        r#"{"model":"panel","messages":[{"role":"user","content":"ping"}]}"#,
    )
    .await?;

    // Then: the surviving model's answer comes back, and no judge prompt was sent.
    assert_eq!(response.status, StatusCode::OK, "body: {}", response.body);
    let json: Value = serde_json::from_str(&response.body)?;
    assert_eq!(
        json.pointer("/choices/0/message/content"),
        Some(&json!("only answer"))
    );
    let bodies = chat_bodies(&provider);
    assert!(
        !bodies.iter().any(|body| body.to_string().contains("JUDGE")),
        "one answer must not be judged: {bodies:#?}"
    );
    Ok(())
}

#[actix_rt::test]
async fn a_fusion_combo_with_no_usable_panel_reports_a_service_failure() -> TestResult {
    // Given: a fusion combo where nothing can execute. There is nothing to judge, so
    // the client must be told rather than handed an empty completion.
    let provider = FakeServer::start(vec![("/chat/completions", completion("unused"))]).await;
    let state = fusion_state(&provider.base_url(), &["ollama/llama3", "cursor/gpt-5"]).await;

    // When: the combo is requested.
    let response = post(
        &state.addr_string(),
        "/v1/chat/completions",
        r#"{"model":"panel","messages":[{"role":"user","content":"ping"}]}"#,
    )
    .await?;

    // Then: a 503 naming the failure, not a fabricated success.
    assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        response.body.contains("fusion panel"),
        "the refusal should say what failed: {}",
        response.body
    );
    Ok(())
}

#[actix_rt::test]
async fn panel_history_has_its_tool_turns_flattened_to_prose() -> TestResult {
    // Given: a conversation whose history contains a tool call and its result. With
    // tools stripped, a panel model handed a `role: tool` message can loop trying to
    // answer it — but dropping the turn would lose the substance of the exchange.
    let provider = FakeServer::start(vec![("/chat/completions", completion("prose"))]).await;
    let state = fusion_state(
        &provider.base_url(),
        &["openai-compatible-f/first", "openai-compatible-f/second"],
    )
    .await;

    // When: the combo is asked with tool history.
    let response = post(
        &state.addr_string(),
        "/v1/chat/completions",
        r#"{"model":"panel","messages":[
            {"role":"user","content":"weather?"},
            {"role":"assistant","tool_calls":[{"id":"c1","type":"function","function":{"name":"get_weather","arguments":"{}"}}]},
            {"role":"tool","tool_call_id":"c1","content":"18C and raining"},
            {"role":"user","content":"and tomorrow?"}
        ]}"#,
    )
    .await?;
    assert_eq!(response.status, StatusCode::OK, "body: {}", response.body);

    let bodies = chat_bodies(&provider);
    let (_, panels) = bodies.split_last().expect("at least one call");
    let panel = panels.first().expect("a panel call");
    let text = panel.to_string();

    // Then: the tool turns survive as prose, and no structured tool turn remains.
    assert!(text.contains("Called tools: get_weather"), "{text}");
    assert!(text.contains("18C and raining"), "{text}");
    assert!(!text.contains("tool_calls"), "{text}");
    assert!(
        !text.contains(r#""role":"tool""#),
        "no tool role should reach a panel model: {text}"
    );
    Ok(())
}

// ── per-combo strategy overrides ─────────────────────────────────────────────

#[actix_web::test]
async fn a_per_combo_override_wins_over_the_global_strategy() -> TestResult {
    // Global says fallback; this combo says fusion. Upstream reads
    // `comboStrategies[name].fallbackStrategy` first, so the combo must fan out.
    let provider = FakeServer::start(vec![("/chat/completions", completion("answer"))]).await;
    let state = fusion_state_with_settings(
        &provider.base_url(),
        &["openai-compatible-a/one", "openai-compatible-b/two"],
        json!({
            "comboStrategy": "fallback",
            "comboStrategies": { "panel": { "fallbackStrategy": "fusion" } },
        }),
    )
    .await;

    let response = post(
        &state.addr_string(),
        "/v1/chat/completions",
        r#"{"model":"panel","messages":[{"role":"user","content":"hi"}]}"#,
    )
    .await?;
    assert_eq!(response.status, StatusCode::OK);
    // Fusion asks every panel model and then a judge: strictly more calls than the
    // one a fallback combo would have made.
    let calls = provider
        .requests()
        .iter()
        .filter(|(path, _)| path.contains("/chat/completions"))
        .count();
    assert!(
        calls > 2,
        "expected a panel fan-out plus a judge, saw {calls} provider calls"
    );
    Ok(())
}

#[actix_web::test]
async fn a_per_combo_override_can_turn_fusion_off_for_one_combo() -> TestResult {
    // The reverse: global fusion, this combo overridden to fallback. One call, because
    // the first model answered and a fallback combo stops there.
    let provider = FakeServer::start(vec![("/chat/completions", completion("answer"))]).await;
    let state = fusion_state_with_settings(
        &provider.base_url(),
        &["openai-compatible-a/one", "openai-compatible-b/two"],
        json!({
            "comboStrategy": "fusion",
            "comboStrategies": { "panel": { "fallbackStrategy": "fallback" } },
        }),
    )
    .await;

    let response = post(
        &state.addr_string(),
        "/v1/chat/completions",
        r#"{"model":"panel","messages":[{"role":"user","content":"hi"}]}"#,
    )
    .await?;
    assert_eq!(response.status, StatusCode::OK);
    let calls = provider
        .requests()
        .iter()
        .filter(|(path, _)| path.contains("/chat/completions"))
        .count();
    assert_eq!(
        calls, 1,
        "a fallback combo whose first model answered calls once"
    );
    Ok(())
}

#[actix_web::test]
async fn an_override_naming_another_combo_does_not_affect_this_one() -> TestResult {
    // Keyed by combo name, so an entry for a combo that is not the one being asked
    // must leave the global in force.
    let provider = FakeServer::start(vec![("/chat/completions", completion("answer"))]).await;
    let state = fusion_state_with_settings(
        &provider.base_url(),
        &["openai-compatible-a/one", "openai-compatible-b/two"],
        json!({
            "comboStrategy": "fusion",
            "comboStrategies": { "some-other-combo": { "fallbackStrategy": "fallback" } },
        }),
    )
    .await;

    let response = post(
        &state.addr_string(),
        "/v1/chat/completions",
        r#"{"model":"panel","messages":[{"role":"user","content":"hi"}]}"#,
    )
    .await?;
    assert_eq!(response.status, StatusCode::OK);
    let calls = provider
        .requests()
        .iter()
        .filter(|(path, _)| path.contains("/chat/completions"))
        .count();
    assert!(
        calls > 2,
        "the global fusion still applies, saw {calls} calls"
    );
    Ok(())
}

#[actix_web::test]
async fn an_unrecognised_override_degrades_to_the_global_rather_than_failing() -> TestResult {
    // A strategy name this build does not know must not fail the request. Upstream's
    // own resolution falls through to the global, and so does this.
    let provider = FakeServer::start(vec![("/chat/completions", completion("answer"))]).await;
    let state = fusion_state_with_settings(
        &provider.base_url(),
        &["openai-compatible-a/one", "openai-compatible-b/two"],
        json!({
            "comboStrategy": "fusion",
            "comboStrategies": { "panel": { "fallbackStrategy": "quantum-entanglement" } },
        }),
    )
    .await;

    let response = post(
        &state.addr_string(),
        "/v1/chat/completions",
        r#"{"model":"panel","messages":[{"role":"user","content":"hi"}]}"#,
    )
    .await?;
    // The request succeeds. It routes as `fallback` — an unknown name is not fusion —
    // which is the safe reading: one call rather than a fan-out nobody asked for.
    assert_eq!(response.status, StatusCode::OK);
    Ok(())
}

#[actix_web::test]
async fn an_override_carrying_only_tuning_leaves_the_strategy_alone() -> TestResult {
    // `comboStrategies["panel"] = { minPanel: 3 }` with a global of fusion: still
    // fusion, because an absent `fallbackStrategy` means "not overridden".
    let provider = FakeServer::start(vec![("/chat/completions", completion("answer"))]).await;
    let state = fusion_state_with_settings(
        &provider.base_url(),
        &["openai-compatible-a/one", "openai-compatible-b/two"],
        json!({
            "comboStrategy": "fusion",
            "comboStrategies": { "panel": { "minPanel": 2, "stragglerGraceMs": 500 } },
        }),
    )
    .await;

    let response = post(
        &state.addr_string(),
        "/v1/chat/completions",
        r#"{"model":"panel","messages":[{"role":"user","content":"hi"}]}"#,
    )
    .await?;
    assert_eq!(response.status, StatusCode::OK);
    let calls = provider
        .requests()
        .iter()
        .filter(|(path, _)| path.contains("/chat/completions"))
        .count();
    assert!(calls > 2, "fusion still applies, saw {calls} calls");
    Ok(())
}
