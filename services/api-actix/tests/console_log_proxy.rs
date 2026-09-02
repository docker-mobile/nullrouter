//! `GET`/`DELETE /api/translator/console-logs` against a stub state service.
//!
//! The rest of this crate's console-log tests point at a closed port, which proves the failure path.
//! This one proves the success path, and specifically the thing the whole design is for: the lines
//! this route returns come out of the state service's buffer rather than a local one. A local buffer
//! would make this list and the events service's stream show different lines while both looked like
//! they worked.

#![allow(clippy::future_not_send)]
#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "free helpers here are not #[test] fns, so clippy.toml's allow-expect-in-tests does \
              not cover them, and indexing a Value is the assertion"
)]

use std::sync::{Arc, Mutex};

use actix_web::{
    App,
    body::to_bytes,
    http::{Method, StatusCode, header},
    test, web,
};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;

use nullrouter_api::{AppConfig, RuntimeClient, StateClient, configure};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A stub state service answering the console-log route, recording the methods it saw.
async fn stub_state(page: Value) -> (String, Arc<Mutex<Vec<String>>>) {
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
            let page = page.clone();
            let recorded = Arc::clone(&recorded);
            tokio::spawn(async move {
                let mut buffer = [0_u8; 4096];
                let read = stream.read(&mut buffer).await.unwrap_or(0);
                let head = String::from_utf8_lossy(buffer.get(..read).unwrap_or_default());
                let request_line = head.lines().next().unwrap_or_default().to_owned();
                recorded
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(request_line.clone());

                let body = if request_line.starts_with("DELETE") {
                    json!({ "success": true }).to_string()
                } else {
                    page.to_string()
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                     Connection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            });
        }
    });

    (addr, seen)
}

async fn call(state_addr: &str, method: Method, uri: &str) -> TestResult<(StatusCode, Value)> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(AppConfig::new("0.5.20")))
            .app_data(web::Data::new(StateClient::new(state_addr)))
            .app_data(web::Data::new(RuntimeClient::new("127.0.0.1:1")))
            .configure(configure),
    )
    .await;
    let request = test::TestRequest::default()
        .method(method)
        .uri(uri)
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .to_request();
    let response = test::call_service(&app, request).await;
    let status = response.status();
    let bytes = to_bytes(response.into_body()).await?;
    Ok((status, serde_json::from_slice(&bytes)?))
}

#[actix_web::test]
async fn the_list_returns_the_lines_the_state_service_holds() -> TestResult {
    // Given: a state service holding lines from two different services.
    let page = json!({
        "logs": [
            "[nullrouter-runtime] info upstream returned 503",
            "[nullrouter-api] warn a provider had no credential",
        ],
        "lines": [
            {"service": "nullrouter-runtime", "level": "info",
             "message": "upstream returned 503", "atMs": 1, "seq": 1},
            {"service": "nullrouter-api", "level": "warn",
             "message": "a provider had no credential", "atMs": 2, "seq": 2},
        ],
        "cursor": 2,
        "generation": 0,
        "dropped": false,
    });
    let (addr, seen) = stub_state(page).await;

    // When: the dashboard asks this service for the buffer.
    let (status, body) = call(&addr, Method::GET, "/api/translator/console-logs").await?;

    // Then: the lines come back, in upstream's `logs: string[]` envelope so an unmodified dashboard
    // renders unchanged.
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["success"], true, "{body}");
    assert_eq!(body["logs"].as_array().map(Vec::len), Some(2), "{body}");
    assert_eq!(
        body["logs"][0],
        "[nullrouter-runtime] info upstream returned 503"
    );

    // And the structured form is carried alongside it, which upstream has no equivalent of: with
    // eight processes writing to one buffer, a bare string is not traceable to what logged it.
    assert_eq!(body["lines"][1]["service"], "nullrouter-api");
    assert_eq!(body["lines"][1]["level"], "warn");
    assert_eq!(body["cursor"], 2, "{body}");

    // And it really went to the state service rather than being answered locally.
    let requests = seen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert!(
        requests
            .iter()
            .any(|line| line.starts_with("GET /internal/v1/console-logs")),
        "{requests:?}"
    );
    Ok(())
}

#[actix_web::test]
async fn the_delete_clears_the_buffer_in_the_state_service() -> TestResult {
    // Given: a reachable state service.
    let (addr, seen) = stub_state(json!({"logs": [], "cursor": 0, "generation": 0})).await;

    // When: the dashboard clears the buffer.
    let (status, body) = call(&addr, Method::DELETE, "/api/translator/console-logs").await?;

    // Then: success is reported, and the clear was forwarded rather than performed on a local buffer
    // the stream does not read.
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["success"], true, "{body}");
    let requests = seen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    assert!(
        requests
            .iter()
            .any(|line| line.starts_with("DELETE /internal/v1/console-logs")),
        "{requests:?}"
    );
    Ok(())
}
