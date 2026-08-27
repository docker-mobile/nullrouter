//! Frames must reach the client incrementally, not after the full completion.
//!
//! This is the difference between usable and unusable for an AI proxy: with a
//! buffered response, time-to-first-token equals the provider's *total* time. A
//! 30-second completion means 30 seconds of blank screen.
//!
//! The provider stub here deliberately stalls between frames, so a buffered
//! implementation cannot pass: the first frame would only be observable after
//! the last one was written.

#![allow(
    clippy::future_not_send,
    clippy::expect_used,
    reason = "test helper: failing to bind a loopback socket should abort the test"
)]

use std::time::{Duration, Instant};

use actix_web::body::MessageBody;
use actix_web::{
    App,
    http::{Method, header},
    test, web,
};
use nullrouter_runtime::{Runtime, app_config, configure};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// Delay the stub inserts between SSE frames.
const FRAME_GAP: Duration = Duration::from_millis(120);
/// Frames the stub emits before `[DONE]`.
const FRAME_COUNT: usize = 4;

/// A provider that emits SSE frames slowly, with a gap between each.
async fn slow_provider() -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("addr").to_string();

    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let _ = serve_slowly(stream).await;
            });
        }
    });

    addr
}

async fn serve_slowly(mut stream: TcpStream) -> std::io::Result<()> {
    // Drain the request head so the client's write completes.
    let mut chunk = [0_u8; 4096];
    let _ = stream.read(&mut chunk).await?;

    // Chunked encoding: the body length is unknown up front, as with a real
    // provider stream.
    stream
        .write_all(
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
        )
        .await?;
    stream.flush().await?;

    for index in 0..FRAME_COUNT {
        tokio::time::sleep(FRAME_GAP).await;
        let payload = json!({
            "id": "chatcmpl-slow",
            "model": "gpt-5",
            "choices": [{ "index": 0, "delta": { "content": format!("f{index}") } }],
        });
        let frame = format!("data: {payload}\n\n");
        stream
            .write_all(format!("{:x}\r\n{frame}\r\n", frame.len()).as_bytes())
            .await?;
        stream.flush().await?;
    }

    let done = "data: [DONE]\n\n";
    stream
        .write_all(format!("{:x}\r\n{done}\r\n0\r\n\r\n", done.len()).as_bytes())
        .await?;
    stream.flush().await
}

/// A state stub handing out credentials that point at `provider_base`.
async fn state_stub(provider_base: String) -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("addr").to_string();

    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let base = provider_base.clone();
            tokio::spawn(async move {
                let _ = serve_state(stream, &base).await;
            });
        }
    });

    addr
}

async fn serve_state(mut stream: TcpStream, provider_base: &str) -> std::io::Result<()> {
    let mut chunk = [0_u8; 8192];
    let read = stream.read(&mut chunk).await?;
    let request = String::from_utf8_lossy(chunk.get(..read).unwrap_or_default()).into_owned();
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/")
        .to_owned();

    let body = if path.contains("/internal/v1/credentials/select") {
        json!({
            "status": "selected",
            "credentials": {
                "connectionId": "conn_slow",
                "connectionName": "slow",
                "apiKey": "sk-slow",
                "providerSpecificData": { "baseUrl": format!("http://{provider_base}") },
            },
        })
        .to_string()
    } else if path.contains("/internal/v1/routing-context") {
        json!({ "combos": [], "connections": [], "settings": {} }).to_string()
    } else {
        json!({ "ok": true }).to_string()
    };

    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await
}

#[actix_rt::test]
async fn first_frame_arrives_before_the_stream_completes() -> TestResult {
    let provider = slow_provider().await;
    let state = state_stub(provider).await;

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(Runtime::with_state_addr(&state)))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::default()
        .method(Method::POST)
        .uri("/v1/chat/completions")
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(
            r#"{"model":"openai-compatible-slow/gpt-5","stream":true,"messages":[{"role":"user","content":"hi"}]}"#,
        )
        .to_request();

    let started = Instant::now();
    let response = test::call_service(&app, req).await;
    assert!(
        response.status().is_success(),
        "status {}",
        response.status()
    );

    // Read one chunk only, then stop: this is the measurement that matters.
    let mut body = response.into_body();
    let mut stream = std::pin::Pin::new(&mut body);
    let first = futures_util::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await;
    let first_frame_at = started.elapsed();

    let Some(Ok(bytes)) = first else {
        panic!("expected a first frame, got {first:?}");
    };
    let text = String::from_utf8_lossy(bytes.as_ref()).into_owned();
    assert!(text.starts_with("data: "), "unexpected frame: {text}");

    // The stub takes FRAME_COUNT * FRAME_GAP to finish. A buffered
    // implementation could not surface a frame before that; an incremental one
    // surfaces the first at roughly one gap.
    let full_stream = FRAME_GAP * u32::try_from(FRAME_COUNT).unwrap_or(1);
    assert!(
        first_frame_at < full_stream,
        "first frame took {first_frame_at:?}, which is not sooner than the \
         full stream duration {full_stream:?} — the response is buffered"
    );
    Ok(())
}

#[actix_rt::test]
async fn every_frame_is_delivered_in_order_and_terminated() -> TestResult {
    let provider = slow_provider().await;
    let state = state_stub(provider).await;

    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(Runtime::with_state_addr(&state)))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::default()
        .method(Method::POST)
        .uri("/v1/chat/completions")
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(
            r#"{"model":"openai-compatible-slow/gpt-5","stream":true,"messages":[{"role":"user","content":"hi"}]}"#,
        )
        .to_request();
    let response = test::call_service(&app, req).await;

    // Incremental delivery must not cost completeness: every frame, in order,
    // with the terminator last.
    let mut collected = Vec::new();
    let mut body = response.into_body();
    let mut stream = std::pin::Pin::new(&mut body);
    while let Some(chunk) = futures_util::future::poll_fn(|cx| stream.as_mut().poll_next(cx)).await
    {
        match chunk {
            Ok(bytes) => collected.extend_from_slice(bytes.as_ref()),
            Err(_) => break,
        }
    }
    let text = String::from_utf8_lossy(&collected).into_owned();

    let contents: Vec<String> = text
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter(|payload| *payload != "[DONE]")
        .filter_map(|payload| serde_json::from_str::<serde_json::Value>(payload).ok())
        .filter_map(|chunk| {
            chunk
                .pointer("/choices/0/delta/content")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .collect();

    assert_eq!(contents, vec!["f0", "f1", "f2", "f3"], "body: {text}");
    assert!(text.trim_end().ends_with("data: [DONE]"), "body: {text}");
    Ok(())
}
