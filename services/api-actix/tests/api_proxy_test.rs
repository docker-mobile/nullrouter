//! `POST /api/settings/proxy-test`: does this proxy actually carry a request?
//!
//! The route answered 501. It now dials a test URL through the given proxy and reports the
//! status, the latency, and — the part that matters — the transport's own error text. A user
//! pasting a corporate proxy URL needs to know whether it refused the connection, timed out, or
//! rejected their credentials; "proxy test failed" sends them guessing.
//!
//! The reachable, refused and timeout cases are exercised against loopback listeners, so the
//! suite needs no egress. The one guard this port adds beyond upstream — refusing a loopback or
//! private *test* URL — is tested here too: the route makes the server dial on a caller's
//! behalf, and a private target reports what the machine can reach rather than what the proxy
//! can.

#![allow(
    clippy::future_not_send,
    clippy::expect_used,
    clippy::panic,
    reason = "test assertions read clearer with direct expect than with error plumbing"
)]

#![allow(
    clippy::indexing_slicing,
    reason = "indexing a serde_json::Value is the assertion: a shape that does not match \
              is a test failure, which is what the panic reports"
)]

use actix_web::{
    App,
    body::to_bytes,
    http::{Method, StatusCode, header},
    test, web,
};
use nullrouter_api::{AppConfig, RuntimeClient, StateClient, configure};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

const UNREACHABLE: &str = "127.0.0.1:1";

const fn app_config() -> AppConfig {
    AppConfig::new("0.5.20")
}

async fn proxy_test(payload: Value) -> TestResult<(StatusCode, Value)> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(StateClient::new(UNREACHABLE)))
            .app_data(web::Data::new(RuntimeClient::new(UNREACHABLE)))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::default()
        .method(Method::POST)
        .uri("/api/settings/proxy-test")
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(payload.to_string())
        .to_request();
    let res = test::call_service(&app, req).await;
    let status = res.status();
    let body = String::from_utf8(to_bytes(res.into_body()).await?.to_vec())?;
    Ok((status, serde_json::from_str(&body)?))
}

/// A loopback listener that behaves like an HTTP proxy for one absolute-form request.
///
/// It does not really proxy: it reads the request line and answers with `reply`. That is enough,
/// because what is under test is how this router reports a proxy's behaviour, not the proxy.
async fn fake_proxy(reply: &'static str) -> TestResult<String> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let reply = reply;
            tokio::spawn(async move {
                let mut buffer = vec![0_u8; 8192];
                let _ = stream.read(&mut buffer).await;
                let _ = stream.write_all(reply.as_bytes()).await;
                let _ = stream.flush().await;
            });
        }
    });
    Ok(format!("http://{addr}"))
}

/// A listener that accepts and then never answers, for the timeout case.
async fn silent_proxy() -> TestResult<String> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            // Held so the connection stays open rather than being closed on drop, which would
            // turn the timeout case into a connection-reset case.
            held.push(stream);
        }
    });
    Ok(format!("http://{addr}"))
}

#[actix_web::test]
async fn a_working_proxy_reports_ok_with_latency() -> TestResult {
    let proxy = fake_proxy("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n").await?;
    let (status, body) = proxy_test(json!({
        "proxyUrl": proxy,
        "testUrl": "http://example.com/",
        "timeoutMs": 4000,
    }))
    .await?;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], true, "{body}");
    assert_eq!(body["status"], 200);
    assert!(body["latencyMs"].is_number(), "{body}");
    assert_eq!(body["testUrl"], "http://example.com/");
    assert_eq!(body["timeoutMs"], 4000);
    assert!(body.get("error").is_none(), "no error expected: {body}");
    Ok(())
}

#[actix_web::test]
async fn a_proxy_that_answers_non_2xx_still_reports_the_status() -> TestResult {
    // The distinction that makes this route useful: the proxy carried the request, and the
    // *site* said no. A user needs to tell that apart from a broken proxy.
    let proxy = fake_proxy("HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n").await?;
    let (status, body) = proxy_test(json!({
        "proxyUrl": proxy,
        "testUrl": "http://example.com/",
        "timeoutMs": 4000,
    }))
    .await?;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], false);
    assert_eq!(body["status"], 403, "the status must survive: {body}");
    let error = body["error"].as_str().unwrap_or_default();
    assert!(error.contains("403"), "{error:?}");
    Ok(())
}

#[actix_web::test]
async fn a_refused_connection_says_so() -> TestResult {
    // Port 1 on loopback is closed.
    let (status, body) = proxy_test(json!({
        "proxyUrl": "http://127.0.0.1:1",
        "testUrl": "http://example.com/",
        "timeoutMs": 3000,
    }))
    .await?;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], false);
    assert!(body.get("status").is_none_or(Value::is_null), "{body}");
    let error = body["error"].as_str().unwrap_or_default().to_lowercase();
    assert!(
        error.contains("connect") || error.contains("refused"),
        "the error should name the connection failure: {error:?}"
    );
    Ok(())
}

#[actix_web::test]
async fn a_timeout_is_reported_as_a_timeout() -> TestResult {
    let proxy = silent_proxy().await?;
    let (status, body) = proxy_test(json!({
        "proxyUrl": proxy,
        "testUrl": "http://example.com/",
        "timeoutMs": 600,
    }))
    .await?;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["ok"], false);
    let error = body["error"].as_str().unwrap_or_default();
    assert!(
        error.to_lowercase().contains("timed out"),
        "expected a timeout, got {error:?}"
    );
    // And the timeout that was actually applied is reported, so a caller can see the clamp.
    assert_eq!(body["timeoutMs"], 600);
    Ok(())
}

#[actix_web::test]
async fn a_missing_proxy_url_is_a_bad_request() -> TestResult {
    for payload in [
        json!({}),
        json!({"proxyUrl": ""}),
        json!({"proxyUrl": "   "}),
    ] {
        let (status, body) = proxy_test(payload.clone()).await?;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{payload} gave {body}");
        assert_eq!(body["ok"], false);
    }
    Ok(())
}

#[actix_web::test]
async fn an_unparseable_proxy_url_names_the_problem() -> TestResult {
    let (status, body) = proxy_test(json!({"proxyUrl": "not a url"})).await?;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let error = body["error"].as_str().unwrap_or_default();
    assert!(error.contains("Invalid proxy URL"), "{error:?}");
    Ok(())
}

#[actix_web::test]
async fn a_local_test_url_is_refused_before_dialling() -> TestResult {
    // This port's own guard. The route makes the server dial for the caller, so a loopback or
    // private target would report what the machine can reach — a network scanner behind a
    // dashboard session. A 400 rather than a failed test, since the proxy is not at fault.
    for target in [
        "http://127.0.0.1:20134/internal/v1/probe-targets",
        "http://localhost:20128/api/settings",
        "http://169.254.169.254/latest/meta-data/",
        "http://10.0.0.1/",
        "http://[::1]/",
    ] {
        let (status, body) = proxy_test(json!({
            "proxyUrl": "http://127.0.0.1:1",
            "testUrl": target,
        }))
        .await?;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{target} gave {body}");
        let error = body["error"].as_str().unwrap_or_default();
        assert!(error.contains("Refusing to dial"), "{target}: {error:?}");
    }
    Ok(())
}

#[actix_web::test]
async fn an_absent_timeout_uses_the_default_and_reports_it() -> TestResult {
    let proxy = fake_proxy("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n").await?;
    let (_, body) = proxy_test(json!({
        "proxyUrl": proxy,
        "testUrl": "http://example.com/",
    }))
    .await?;
    // Upstream's default is 8s.
    assert_eq!(body["timeoutMs"], 8000, "{body}");
    Ok(())
}

#[actix_web::test]
async fn an_oversized_timeout_is_clamped() -> TestResult {
    let proxy = fake_proxy("HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n").await?;
    let (_, body) = proxy_test(json!({
        "proxyUrl": proxy,
        "testUrl": "http://example.com/",
        "timeoutMs": 600_000,
    }))
    .await?;
    // 30s ceiling, so one request cannot hold a worker for ten minutes.
    assert_eq!(body["timeoutMs"], 30_000, "{body}");
    Ok(())
}

#[actix_web::test]
async fn the_default_test_url_is_used_and_reported() -> TestResult {
    // No egress needed: the proxy is loopback and refuses, so the dial fails — but the *chosen*
    // URL is still reported, which is what this asserts.
    let (_, body) = proxy_test(json!({"proxyUrl": "http://127.0.0.1:1"})).await?;
    assert_eq!(body["testUrl"], "https://google.com/", "{body}");
    Ok(())
}

#[actix_web::test]
async fn the_configuration_export_refuses_rather_than_returning_an_empty_backup() -> TestResult {
    // It used to answer `success: true` with empty arrays: a file that looked like a backup,
    // validated like a backup, and contained none of the user's providers or keys. They would
    // find out when they tried to restore it.
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(StateClient::new(UNREACHABLE)))
            .app_data(web::Data::new(RuntimeClient::new(UNREACHABLE)))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::default()
        .method(Method::GET)
        .uri("/api/settings/database")
        .to_request();
    let res = test::call_service(&app, req).await;
    let status = res.status();
    let body: Value = serde_json::from_str(&String::from_utf8(
        to_bytes(res.into_body()).await?.to_vec(),
    )?)?;

    assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{body}");
    assert_eq!(body["success"], false);
    assert_eq!(body["unsupported"], true);
    let error = body["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("credential"),
        "the refusal should say why: {error:?}"
    );
    Ok(())
}
