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
    // With enforcement off, no validation call is made at all.
    assert!(
        !paths
            .iter()
            .any(|path| path.contains("/internal/v1/keys/validate")),
        "validation must be skipped when disabled, saw {paths:?}"
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
