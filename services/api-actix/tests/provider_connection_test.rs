//! `POST /api/providers/{id}/test` and its siblings make real upstream calls.
//!
//! These routes used to answer `501 unsupported`. The contract now is: only an
//! upstream 2xx reports success, a provider's refusal is relayed with its own
//! (scrubbed) message rather than flattened into a generic failure, and the number
//! of billable calls one dashboard click can trigger is bounded.

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
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// Nothing listens on port 1, so a client must report it as unreachable.
const DEAD_ADDR: &str = "127.0.0.1:1";

const fn app_config() -> AppConfig {
    AppConfig::new("0.5.20")
}

/// One canned reply. A `path` ending in `/` matches by prefix, so connection ids
/// can be served without enumerating them.
#[derive(Clone)]
struct Route {
    path: &'static str,
    status: u16,
    body: String,
}

/// A stub HTTP server that answers from `routes` and records every request.
///
/// Both the state service and the runtime are stubbed this way, so a test can
/// assert on exactly what the probe sent upstream.
async fn stub(routes: Vec<Route>) -> (String, Arc<Mutex<Vec<String>>>) {
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
            let table = routes.clone();
            let sink = Arc::clone(&recorded);
            tokio::spawn(async move {
                let _ = serve(stream, &table, sink).await;
            });
        }
    });

    (addr, seen)
}

async fn serve(
    mut stream: TcpStream,
    routes: &[Route],
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

    let matched = routes.iter().find(|route| {
        route.path.strip_suffix('/').map_or_else(
            || path == route.path,
            |prefix| path.starts_with(prefix) && path.len() > prefix.len(),
        )
    });
    let (status, reply) = matched.map_or_else(
        || (404_u16, r#"{"error":"no stub route"}"#.to_owned()),
        |route| (route.status, route.body.clone()),
    );

    let response = format!(
        "HTTP/1.1 {status} STUB\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
        reply.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await
}

/// A state reply for one connection read.
fn connection_route(provider: &str, model: &str) -> Route {
    Route {
        path: "/api/providers/",
        status: 200,
        body: serde_json::json!({
            "connection": {
                "id": "conn-1",
                "provider": provider,
                "defaultModel": model,
                "hasApiKey": true,
            }
        })
        .to_string(),
    }
}

/// A runtime reply that looks like a normal completion.
fn chat_ok() -> Route {
    Route {
        path: "/v1/chat/completions",
        status: 200,
        body: r#"{"id":"chatcmpl-1","choices":[{"message":{"role":"assistant","content":"."}}]}"#
            .to_owned(),
    }
}

async fn post(
    state_addr: &str,
    runtime_addr: &str,
    uri: &str,
    payload: &str,
) -> TestResult<(StatusCode, Value)> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(StateClient::new(state_addr)))
            .app_data(web::Data::new(RuntimeClient::new(runtime_addr)))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::default()
        .method(Method::POST)
        .uri(uri)
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(payload.to_owned())
        .to_request();
    let res = test::call_service(&app, req).await;
    let status = res.status();
    let raw = String::from_utf8(to_bytes(res.into_body()).await?.to_vec())?;
    let json = serde_json::from_str::<Value>(&raw).unwrap_or(Value::String(raw));
    Ok((status, json))
}

#[actix_rt::test]
async fn a_working_connection_reports_the_call_it_actually_made() -> TestResult {
    let (state_addr, _) = stub(vec![connection_route("openai", "gpt-5")]).await;
    let (runtime_addr, runtime_seen) = stub(vec![chat_ok()]).await;

    let (status, body) = post(
        &state_addr,
        &runtime_addr,
        "/api/providers/conn-1/test",
        "{}",
    )
    .await?;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body.get("success").and_then(Value::as_bool), Some(true));
    assert_eq!(body.get("valid").and_then(Value::as_bool), Some(true));
    assert_eq!(body.get("status").and_then(Value::as_u64), Some(200));
    assert!(
        body.get("error").is_none(),
        "a pass carries no error: {body}"
    );
    // The model tested is reported, so the user knows what was proven.
    assert_eq!(
        body.get("model").and_then(Value::as_str),
        Some("openai/gpt-5")
    );

    // The probe really went upstream, and it is the cheap shape.
    let calls = runtime_seen
        .lock()
        .map(|seen| seen.clone())
        .unwrap_or_default();
    let probe = calls.first().expect("runtime was called");
    assert!(probe.contains("/v1/chat/completions"), "got {probe}");
    assert!(probe.contains("\"max_tokens\":1"), "got {probe}");
    assert!(probe.contains("\"stream\":false"), "got {probe}");
    assert!(probe.contains("\"model\":\"openai/gpt-5\""), "got {probe}");
    Ok(())
}

#[actix_rt::test]
async fn a_rejected_key_fails_the_test_and_never_echoes_the_key() -> TestResult {
    let (state_addr, _) = stub(vec![connection_route("openai", "gpt-5")]).await;
    let (runtime_addr, _) = stub(vec![Route {
        path: "/v1/chat/completions",
        status: 401,
        body: r#"{"error":{"message":"Incorrect API key provided: sk-live-AAAABBBBCCCCDDDD1111"}}"#
            .to_owned(),
    }])
    .await;

    let (status, body) = post(
        &state_addr,
        &runtime_addr,
        "/api/providers/conn-1/test",
        "{}",
    )
    .await?;

    // The route ran; the provider refused. 200 here would make a dead key look live.
    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_eq!(body.get("success").and_then(Value::as_bool), Some(false));
    assert_eq!(body.get("status").and_then(Value::as_u64), Some(401));

    let error = body
        .get("error")
        .and_then(Value::as_str)
        .expect("a failure carries a message");
    // The provider's own complaint survives, because "bad key" and "bad model"
    // send the user to different places.
    assert!(error.contains("Incorrect API key"), "got {error}");
    // The key itself does not.
    assert!(
        !error.contains("sk-live-AAAABBBBCCCCDDDD1111"),
        "the key leaked into a dashboard message: {error}"
    );
    assert!(error.contains("[redacted]"), "got {error}");
    Ok(())
}

#[actix_rt::test]
async fn an_unreachable_provider_path_is_a_failure_not_a_pass() -> TestResult {
    let (state_addr, _) = stub(vec![connection_route("openai", "gpt-5")]).await;

    let (status, body) = post(&state_addr, DEAD_ADDR, "/api/providers/conn-1/test", "{}").await?;

    assert_eq!(status, StatusCode::BAD_GATEWAY, "{body}");
    assert_eq!(body.get("success").and_then(Value::as_bool), Some(false));
    // No upstream status, because the call never landed — and that is not a 200.
    assert!(body.get("status").is_none(), "{body}");
    assert!(
        body.get("error").and_then(Value::as_str).is_some(),
        "{body}"
    );
    Ok(())
}

#[actix_rt::test]
async fn an_unknown_connection_is_a_404_and_costs_nothing() -> TestResult {
    // State answers 404 for every path.
    let (state_addr, _) = stub(Vec::new()).await;
    let (runtime_addr, runtime_seen) = stub(vec![chat_ok()]).await;

    let (status, body) = post(&state_addr, &runtime_addr, "/api/providers/nope/test", "{}").await?;

    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(body.get("success").and_then(Value::as_bool), Some(false));
    // Nothing billable happened for a connection that does not exist.
    let calls = runtime_seen
        .lock()
        .map(|seen| seen.clone())
        .unwrap_or_default();
    assert!(calls.is_empty(), "unexpected upstream calls: {calls:?}");
    Ok(())
}

#[actix_rt::test]
async fn a_connection_with_no_model_is_rejected_before_calling_out() -> TestResult {
    let (state_addr, _) = stub(vec![Route {
        path: "/api/providers/",
        status: 200,
        // A provider with no default model and no registry entry to fall back on.
        body: r#"{"connection":{"id":"conn-1","provider":"not-a-real-provider"}}"#.to_owned(),
    }])
    .await;
    let (runtime_addr, runtime_seen) = stub(vec![chat_ok()]).await;

    let (status, body) = post(
        &state_addr,
        &runtime_addr,
        "/api/providers/conn-1/test",
        "{}",
    )
    .await?;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    let calls = runtime_seen
        .lock()
        .map(|seen| seen.clone())
        .unwrap_or_default();
    assert!(calls.is_empty(), "unexpected upstream calls: {calls:?}");
    Ok(())
}

#[actix_rt::test]
async fn model_testing_is_bounded_and_summarised() -> TestResult {
    let (state_addr, _) = stub(vec![connection_route("openai", "gpt-5")]).await;
    let (runtime_addr, runtime_seen) = stub(vec![chat_ok()]).await;

    // Nine models asked for; the cap is five, because each one is a real call.
    let requested = (1..=9)
        .map(|index| format!("\"m{index}\""))
        .collect::<Vec<_>>()
        .join(",");
    let (status, body) = post(
        &state_addr,
        &runtime_addr,
        "/api/providers/conn-1/test-models",
        &format!("{{\"models\":[{requested}]}}"),
    )
    .await?;

    assert_eq!(status, StatusCode::OK, "{body}");
    let results = body
        .get("results")
        .and_then(Value::as_array)
        .expect("results array");
    assert_eq!(results.len(), 5, "the cap must hold: {body}");
    assert_eq!(
        body.pointer("/summary/passed").and_then(Value::as_u64),
        Some(5)
    );
    assert_eq!(
        body.pointer("/summary/failed").and_then(Value::as_u64),
        Some(0)
    );
    // Bare names are qualified with the connection's provider.
    assert_eq!(
        results
            .first()
            .and_then(|entry| entry.get("model"))
            .and_then(Value::as_str),
        Some("openai/m1")
    );
    let calls = runtime_seen
        .lock()
        .map(|seen| seen.clone())
        .unwrap_or_default();
    assert_eq!(calls.len(), 5, "exactly five billable calls: {calls:?}");
    Ok(())
}

#[actix_rt::test]
async fn a_batch_tests_the_connections_its_mode_selects() -> TestResult {
    let (state_addr, _) = stub(vec![Route {
        path: "/api/providers",
        status: 200,
        body: serde_json::json!({
            "connections": [
                { "id": "a", "provider": "openai", "defaultModel": "gpt-5", "hasApiKey": true },
                { "id": "b", "provider": "anthropic", "defaultModel": "claude-sonnet-4-5",
                  "hasAccessToken": true },
            ]
        })
        .to_string(),
    }])
    .await;
    let (runtime_addr, runtime_seen) = stub(vec![chat_ok()]).await;

    // `oauth` covers only the token-bearing connection.
    let (status, body) = post(
        &state_addr,
        &runtime_addr,
        "/api/providers/test-batch",
        r#"{"mode":"oauth"}"#,
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{body}");
    let results = body
        .get("results")
        .and_then(Value::as_array)
        .expect("results array");
    assert_eq!(results.len(), 1, "{body}");
    assert_eq!(
        results
            .first()
            .and_then(|entry| entry.get("connectionId"))
            .and_then(Value::as_str),
        Some("b")
    );
    let calls = runtime_seen
        .lock()
        .map(|seen| seen.clone())
        .unwrap_or_default();
    assert_eq!(
        calls.len(),
        1,
        "one selected connection, one call: {calls:?}"
    );
    Ok(())
}

#[actix_rt::test]
async fn a_batch_rejects_a_mode_it_cannot_honour() -> TestResult {
    let (state_addr, _) = stub(Vec::new()).await;
    let (runtime_addr, runtime_seen) = stub(vec![chat_ok()]).await;

    for payload in ["{}", r#"{"mode":"whatever"}"#] {
        let (status, body) = post(
            &state_addr,
            &runtime_addr,
            "/api/providers/test-batch",
            payload,
        )
        .await?;
        assert_eq!(status, StatusCode::BAD_REQUEST, "for {payload}: {body}");
    }

    let calls = runtime_seen
        .lock()
        .map(|seen| seen.clone())
        .unwrap_or_default();
    assert!(calls.is_empty(), "unexpected upstream calls: {calls:?}");
    Ok(())
}
