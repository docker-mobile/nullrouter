//! Byte-preserving dispatch, as the async video endpoints need it.
//!
//! `execute_raw` exists because video edits and extensions accept multipart bodies.
//! Parsing a multipart body and re-encoding it mints a new boundary that no longer
//! matches the client's `Content-Type`, so the provider rejects it. Every assertion
//! here is about the bytes and headers arriving unchanged.

mod mock_upstream;

use mock_upstream::{MockResponse, MockUpstream};
use nullrouter_execute::credentials::Credentials;
use nullrouter_execute::{Executor, RawRequest};
use serde_json::json;

fn credentials() -> Credentials {
    Credentials {
        api_key: Some("sk-video-test".to_owned()),
        connection_id: "conn_video".to_owned(),
        connection_name: "video".to_owned(),
        ..Credentials::default()
    }
}

#[tokio::test]
async fn a_multipart_body_arrives_byte_for_byte_with_its_own_boundary() {
    let server = MockUpstream::start(vec![MockResponse::json(
        200,
        r#"{"request_id":"vid_1","status":"pending"}"#,
    )])
    .await;
    let url = format!("http://{}/v1/videos/edits", server.addr);

    // A real multipart body, boundary included. If anything re-encodes this, the
    // boundary in the header and the one in the body stop matching.
    let boundary = "----nullrouterBoundary7MA4YWxkTrZu0gW";
    let body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"prompt\"\r\n\r\nmake it rain\r\n--{boundary}--\r\n"
    );
    let content_type = format!("multipart/form-data; boundary={boundary}");

    let outcome = Executor::new()
        .execute_raw(RawRequest {
            provider: "xai",
            url: &url,
            post: true,
            body: body.as_bytes(),
            content_type: Some(&content_type),
            extra_headers: &[("Idempotency-Key", "idem-1")],
            credentials: &credentials(),
        })
        .await
        .expect("dispatch");
    assert_eq!(outcome.status().as_u16(), 200);

    let requests = server.requests();
    let sent = requests.first().expect("upstream was called");
    assert_eq!(sent.method, "POST");
    // The exact bytes, not a re-encoding.
    assert_eq!(sent.body, body, "the multipart body was altered in transit");
    // The client's content type travels with them, boundary intact.
    assert_eq!(
        sent.headers.get("content-type").map(String::as_str),
        Some(content_type.as_str()),
        "the boundary must match the body: {:?}",
        sent.headers.get("content-type")
    );
    // Per-request headers survive the provider's own.
    assert_eq!(
        sent.headers.get("idempotency-key").map(String::as_str),
        Some("idem-1")
    );
    // The credential is still applied.
    assert_eq!(
        sent.headers.get("authorization").map(String::as_str),
        Some("Bearer sk-video-test")
    );
}

#[tokio::test]
async fn a_poll_is_a_get_with_no_body_and_no_content_type() {
    let server = MockUpstream::start(vec![MockResponse::json(
        200,
        r#"{"request_id":"vid_1","status":"completed","video":{"url":"https://x"}}"#,
    )])
    .await;
    let url = format!("http://{}/v1/videos/vid_1", server.addr);

    let outcome = Executor::new()
        .execute_raw(RawRequest {
            provider: "xai",
            url: &url,
            post: false,
            // Even given bytes, a GET must not send them.
            body: b"ignored",
            content_type: None,
            extra_headers: &[],
            credentials: &credentials(),
        })
        .await
        .expect("dispatch");
    assert_eq!(outcome.status().as_u16(), 200);

    let requests = server.requests();
    let sent = requests.first().expect("upstream was called");
    assert_eq!(sent.method, "GET");
    assert!(sent.body.is_empty(), "a poll sent a body: {}", sent.body);
    // No content type is declared for a request with no content.
    assert_eq!(sent.headers.get("content-type"), None);
    assert_eq!(sent.path, "/v1/videos/vid_1");
}

#[tokio::test]
async fn a_json_body_keeps_its_declared_content_type() {
    let server =
        MockUpstream::start(vec![MockResponse::json(200, r#"{"request_id":"vid_2"}"#)]).await;
    let url = format!("http://{}/v1/videos/generations", server.addr);
    let body = json!({ "model": "grok-imagine-video", "prompt": "a cat" }).to_string();

    Executor::new()
        .execute_raw(RawRequest {
            provider: "xai",
            url: &url,
            post: true,
            body: body.as_bytes(),
            content_type: Some("application/json"),
            extra_headers: &[],
            credentials: &credentials(),
        })
        .await
        .expect("dispatch");

    let requests = server.requests();
    let sent = requests.first().expect("upstream was called");
    assert_eq!(sent.body, body);
    assert_eq!(
        sent.headers.get("content-type").map(String::as_str),
        Some("application/json")
    );
    // A JSON reply is what this endpoint expects back.
    assert_eq!(
        sent.headers.get("accept").map(String::as_str),
        Some("application/json")
    );
}

#[tokio::test]
async fn an_upstream_refusal_is_returned_rather_than_retried() {
    // Two queued replies: a 500 then a 200. `execute_raw` performs exactly one
    // request, so the 200 must never be reached — a creation POST that upstream
    // 500s may already have created a billable job.
    let server = MockUpstream::start(vec![
        MockResponse::json(500, r#"{"error":"internal"}"#),
        MockResponse::json(200, r#"{"request_id":"vid_3"}"#),
    ])
    .await;
    let url = format!("http://{}/v1/videos/generations", server.addr);

    let outcome = Executor::new()
        .execute_raw(RawRequest {
            provider: "xai",
            url: &url,
            post: true,
            body: b"{}",
            content_type: Some("application/json"),
            extra_headers: &[],
            credentials: &credentials(),
        })
        .await
        .expect("dispatch");

    assert_eq!(outcome.status().as_u16(), 500);
    assert_eq!(
        server.request_count(),
        1,
        "a creation POST must be sent exactly once"
    );
}
