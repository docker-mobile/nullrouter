//! `requireApiKey` enforcement.
//!
//! The gateway's managed-key flag is static configuration, but upstream reads
//! this from persisted settings. Without enforcement at the runtime, enabling
//! the setting in the dashboard would be silently ignored — a user could believe
//! their local API was protected when it was not. These tests pin the gate.

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

/// A state stub that reports `requireApiKey` and validates one known key.
async fn state_stub(
    require_api_key: bool,
    valid_key: &'static str,
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
                let _ = serve(stream, require_api_key, valid_key, sink).await;
            });
        }
    });

    (addr, seen)
}

/// A state stub whose `requireApiKey` answer changes between admission decisions.
///
/// The first answer reports the gate off; the second models the dashboard toggling it on. The
/// runtime must ask state on every request rather than reusing any cached routing context.
async fn toggling_state_stub(valid_key: &'static str) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("addr").to_string();
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&seen);
    let contexts = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let sink = Arc::clone(&recorded);
            let contexts = Arc::clone(&contexts);
            tokio::spawn(async move {
                let _ = serve_toggling(stream, valid_key, sink, contexts).await;
            });
        }
    });
    (addr, seen)
}

async fn serve_toggling(
    mut stream: TcpStream,
    valid_key: &str,
    seen: Arc<Mutex<Vec<String>>>,
    contexts: Arc<std::sync::atomic::AtomicUsize>,
) -> std::io::Result<()> {
    let mut buffer = Vec::new();
    let mut chunk = [0_u8; 8192];
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
    if let Ok(mut sink) = seen.lock() {
        sink.push(path.clone());
    }
    let reply = if path.contains("/internal/v1/keys/gate") {
        // First answer says public; every later one says the dashboard turned the gate on.
        let required = contexts.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0;
        let body = raw.get(head_end..).unwrap_or_default();
        let presented = serde_json::from_str::<Value>(body)
            .ok()
            .and_then(|value| {
                value
                    .get("apiKey")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_default();
        let matches = presented == valid_key;
        json!({"requireApiKey": required, "valid": matches, "active": matches}).to_string()
    } else if path.contains("/internal/v1/routing-context") {
        json!({"combos": [], "connections": [], "settings": {"requireApiKey": false}}).to_string()
    } else if path.contains("/internal/v1/keys/validate") {
        let body = raw.get(head_end..).unwrap_or_default();
        let presented = serde_json::from_str::<Value>(body)
            .ok()
            .and_then(|value| {
                value
                    .get("apiKey")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_default();
        let matches = presented == valid_key;
        json!({"valid": matches, "active": matches}).to_string()
    } else if path.contains("/internal/v1/credentials/select") {
        json!({"status":"no_credentials","message":"gate passed"}).to_string()
    } else {
        json!({"ok":true}).to_string()
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        reply.len(),
        reply
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await
}

async fn serve(
    mut stream: TcpStream,
    require_api_key: bool,
    valid_key: &str,
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
        sink.push(path.clone());
    }

    let reply = if path.contains("/internal/v1/routing-context") {
        json!({
            "combos": [],
            "connections": [],
            "settings": { "requireApiKey": require_api_key },
        })
        .to_string()
    } else if path.contains("/internal/v1/keys/gate") {
        // One call answers both halves, as the real state service does.
        let presented = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|parsed| {
                parsed
                    .get("apiKey")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_default();
        let matches = presented == valid_key;
        json!({
            "requireApiKey": require_api_key,
            "valid": matches,
            "active": matches,
        })
        .to_string()
    } else if path.contains("/internal/v1/keys/validate") {
        let presented = serde_json::from_str::<Value>(&body)
            .ok()
            .and_then(|parsed| {
                parsed
                    .get("apiKey")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_default();
        let matches = presented == valid_key;
        json!({ "valid": matches, "active": matches }).to_string()
    } else if path.contains("/internal/v1/credentials/select") {
        // Reaching selection means the gate let the request through.
        json!({ "status": "no_credentials", "message": "gate passed" }).to_string()
    } else {
        json!({ "ok": true }).to_string()
    };

    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        reply.len(),
        reply
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await
}

/// A state stub whose gate route fails while every other route answers normally.
///
/// Returns the paths it was asked for, so a test can assert what was *not* reached. Without that,
/// a test can only see the 503 — and a 503 is also what a bypassed request would produce once it
/// reached credential selection and that failed, so the status alone proves nothing about ordering.
async fn failing_gate_stub() -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("addr").to_string();
    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&seen);
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                break;
            };
            let sink = Arc::clone(&recorded);
            tokio::spawn(async move {
                let mut buffer = [0_u8; 8192];
                let read = stream.read(&mut buffer).await.unwrap_or(0);
                let raw = String::from_utf8_lossy(buffer.get(..read).unwrap_or_default());
                let path = raw
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/")
                    .to_owned();
                if let Ok(mut paths) = sink.lock() {
                    paths.push(path.clone());
                }
                let response = if path.contains("/internal/v1/keys/gate") {
                    "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .to_owned()
                } else {
                    // A *successful* selection, so that a request which wrongly got past the gate
                    // would proceed rather than fail here. The failure has to be the gate's alone.
                    let body = json!({
                        "status": "selected",
                        "credentials": {
                            "connectionId": "conn-1",
                            "provider": "openai",
                            "apiKey": "sk-upstream",
                            "baseUrl": "http://127.0.0.1:1",
                        },
                    })
                    .to_string();
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    )
                };
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.flush().await;
            });
        }
    });
    (addr, seen)
}

struct Response {
    status: StatusCode,
    body: String,
}

/// Post a chat request, optionally with an auth header.
async fn post_with_key(state_addr: &str, auth: Option<(&str, &str)>) -> TestResult<Response> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(Runtime::with_state_addr(state_addr)))
            .configure(configure),
    )
    .await;
    let mut req = test::TestRequest::default()
        .method(Method::POST)
        .uri("/v1/chat/completions")
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(r#"{"model":"openai/gpt-5","messages":[{"role":"user","content":"hi"}]}"#);
    if let Some((name, value)) = auth {
        req = req.insert_header((name, value));
    }
    let res = test::call_service(&app, req.to_request()).await;
    let status = res.status();
    let body = String::from_utf8(to_bytes(res.into_body()).await?.to_vec())?;
    Ok(Response { status, body })
}

#[actix_rt::test]
async fn request_without_a_key_is_rejected_when_required() -> TestResult {
    let (addr, _) = state_stub(true, "sk-valid").await;

    let response = post_with_key(&addr, None).await?;

    assert_eq!(
        response.status,
        StatusCode::UNAUTHORIZED,
        "{}",
        response.body
    );
    let json: Value = serde_json::from_str(&response.body)?;
    assert_eq!(
        json.pointer("/error/message"),
        Some(&json!("Missing API key"))
    );
    Ok(())
}

#[actix_rt::test]
async fn invalid_key_is_rejected_when_required() -> TestResult {
    let (addr, _) = state_stub(true, "sk-valid").await;

    let response = post_with_key(&addr, Some(("authorization", "Bearer sk-wrong"))).await?;

    assert_eq!(
        response.status,
        StatusCode::UNAUTHORIZED,
        "{}",
        response.body
    );
    let json: Value = serde_json::from_str(&response.body)?;
    assert_eq!(
        json.pointer("/error/message"),
        Some(&json!("Invalid API key"))
    );
    Ok(())
}

#[actix_rt::test]
async fn valid_bearer_key_passes_the_gate() -> TestResult {
    let (addr, seen) = state_stub(true, "sk-valid").await;

    let response = post_with_key(&addr, Some(("authorization", "Bearer sk-valid"))).await?;

    // Past the gate, the request proceeds to credential selection, which this
    // stub answers with "no credentials".
    assert_ne!(
        response.status,
        StatusCode::UNAUTHORIZED,
        "{}",
        response.body
    );
    let paths = seen.lock().map(|seen| seen.clone()).unwrap_or_default();
    assert!(
        paths
            .iter()
            .any(|path| path.contains("/internal/v1/credentials/select")),
        "gate should have allowed selection, saw {paths:?}"
    );
    Ok(())
}

#[actix_rt::test]
async fn anthropic_x_api_key_header_is_accepted() -> TestResult {
    let (addr, _) = state_stub(true, "sk-valid").await;

    // Claude clients send the key in x-api-key rather than Authorization.
    let response = post_with_key(&addr, Some(("x-api-key", "sk-valid"))).await?;

    assert_ne!(
        response.status,
        StatusCode::UNAUTHORIZED,
        "{}",
        response.body
    );
    Ok(())
}

#[actix_rt::test]
async fn no_key_is_needed_when_the_setting_is_off() -> TestResult {
    let (addr, seen) = state_stub(false, "sk-valid").await;

    let response = post_with_key(&addr, None).await?;

    assert_ne!(
        response.status,
        StatusCode::UNAUTHORIZED,
        "{}",
        response.body
    );
    let paths = seen.lock().map(|seen| seen.clone()).unwrap_or_default();
    // The one-call gate still reads the live requirement setting, but never makes the old second
    // validation call when the setting is disabled.
    assert!(
        paths
            .iter()
            .any(|path| path.contains("/internal/v1/keys/gate")),
        "the live gate must be consulted, saw {paths:?}"
    );
    assert!(
        !paths
            .iter()
            .any(|path| path.contains("/internal/v1/keys/validate")),
        "the retired validation hop must not be used, saw {paths:?}"
    );
    Ok(())
}

#[actix_rt::test]
async fn enforcement_fails_closed_when_state_is_unreachable() -> TestResult {
    // A stub that reports enforcement on, then a runtime pointed at a dead port
    // for validation, must not fall open.
    let (addr, _) = state_stub(true, "sk-valid").await;
    let response = post_with_key(&addr, Some(("authorization", "Bearer sk-valid"))).await?;
    assert_ne!(response.status, StatusCode::UNAUTHORIZED);

    // Now the whole state service is gone: routing context defaults to
    // requireApiKey=false, so the request proceeds and fails later on
    // credentials rather than being silently authorized as a valid key.
    let dead = post_with_key("127.0.0.1:1", None).await?;
    assert_eq!(
        dead.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        dead.body
    );
    Ok(())
}

#[actix_rt::test]
async fn gate_failure_denies_before_credential_selection() -> TestResult {
    // When the one-call gate cannot answer, the request must be refused rather than proceeding on
    // an assumed-public setting. The earlier version of this returned `None` — continue — which
    // inferred "no key required" from *no answer at all* rather than from an answer saying so.
    //
    // The stub fails only `/keys/gate` and answers credential selection successfully, so a request
    // that wrongly got past the gate would carry on rather than fail. Asserting the 503 alone would
    // not prove ordering: a bypassed request that later failed selection also ends in a 503. The
    // load-bearing assertion is that selection was never asked.
    let (addr, seen) = failing_gate_stub().await;
    let response = post_with_key(&addr, None).await?;
    assert_eq!(
        response.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        response.body
    );
    assert!(
        response.body.contains("API-key gate unavailable"),
        "the refusal should name the gate, not a downstream failure: {}",
        response.body
    );

    let paths = seen.lock().map(|paths| paths.clone()).unwrap_or_default();
    assert!(
        paths
            .iter()
            .any(|path| path.contains("/internal/v1/keys/gate")),
        "the gate must have been consulted: {paths:?}"
    );
    assert!(
        !paths
            .iter()
            .any(|path| path.contains("/internal/v1/credentials/select")),
        "a failed gate must stop the request before credential selection: {paths:?}"
    );
    Ok(())
}

#[actix_rt::test]
async fn toggle_on_is_not_bypassed_by_a_cached_public_context() -> TestResult {
    // The bypass a reviewer found. `StateClient` caches routing context for 250ms, and that context
    // carries `requireApiKey`. If the gate reads it from the cache, then a dashboard toggling the
    // gate on leaves /v1 open for up to the TTL -- and when the gateway's static
    // NULLROUTER_REQUIRE_API_KEY is off, nothing else is checking.
    //
    // One `Runtime` for both requests, deliberately: a fresh Runtime per request has an empty cache
    // and cannot reproduce this. The stub reports requireApiKey=false on its first admission
    // decision and true afterwards, so request one seeds a "public" cache entry and request two
    // must still be rejected.
    let (addr, seen) = toggling_state_stub("sk-valid").await;
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(Runtime::with_state_addr(&addr)))
            .configure(configure),
    )
    .await;

    let unauthenticated = || {
        test::TestRequest::default()
            .method(Method::POST)
            .uri("/v1/chat/completions")
            .insert_header((header::CONTENT_TYPE, "application/json"))
            .set_payload(r#"{"model":"openai/gpt-5","messages":[{"role":"user","content":"hi"}]}"#)
            .to_request()
    };

    let first = test::call_service(&app, unauthenticated()).await;
    assert_ne!(
        first.status(),
        StatusCode::UNAUTHORIZED,
        "the first context reports the gate off, so this one passes"
    );

    let second = test::call_service(&app, unauthenticated()).await;
    let status = second.status();
    let body = String::from_utf8(to_bytes(second.into_body()).await?.to_vec())?;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "a toggle-on must take effect immediately, not after the cache TTL: {body}"
    );

    let paths = seen.lock().map(|paths| paths.clone()).unwrap_or_default();
    assert!(
        paths
            .iter()
            .filter(|path| path.contains("/internal/v1/keys/gate"))
            .count()
            >= 2,
        "the gate must ask state on each request rather than reuse a cached setting: {paths:?}"
    );
    Ok(())
}
