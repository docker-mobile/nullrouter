#![allow(
    clippy::future_not_send,
    clippy::expect_used,
    reason = "test helper: failing to bind a loopback socket should abort the test"
)]

use actix_web::{
    App,
    body::to_bytes,
    http::{Method, StatusCode, header},
    test, web,
};
use serde_json::Value;

use nullrouter_runtime::{Runtime, app_config, configure};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

#[path = "support/public_gate.rs"]
mod public_gate;

struct RuntimeResponse {
    status: StatusCode,
    content_type: String,
    body: String,
}

async fn request(method: Method, uri: &str, body: &str) -> TestResult<RuntimeResponse> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(Runtime::with_state_addr(
                &public_gate::start().await,
            )))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::default()
        .method(method)
        .uri(uri)
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(body.to_owned())
        .to_request();
    let res = test::call_service(&app, req).await;
    let status = res.status();
    let content_type = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = String::from_utf8(to_bytes(res.into_body()).await?.to_vec())?;
    Ok(RuntimeResponse {
        status,
        content_type,
        body,
    })
}

fn parse_json(body: &str) -> TestResult<Value> {
    Ok(serde_json::from_str(body)?)
}

fn string_at<'a>(json: &'a Value, pointer: &str) -> TestResult<&'a str> {
    json.pointer(pointer)
        .and_then(Value::as_str)
        .ok_or_else(|| test_error(format!("missing string at {pointer}")))
}

#[actix_rt::test]
async fn media_provider_posts_reject_malformed_json_with_structured_errors() -> TestResult {
    // Given: upstream media-provider test endpoints parse JSON at the runtime boundary.
    let routes = [
        "/v1/embeddings",
        "/v1/audio/speech",
        "/v1/images/generations",
        "/v1/search",
        "/v1/web/fetch",
    ];

    // When: malformed JSON is posted to each endpoint.
    for route in routes {
        let response = request(Method::POST, route, "{").await?;
        let json = parse_json(&response.body)?;

        // Then: the runtime returns a structured JSON error instead of framework HTML or SSE.
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{route}");
        assert!(
            response.content_type.starts_with("application/json"),
            "{route}"
        );
        assert!(!response.content_type.starts_with("text/event-stream"));
        assert_eq!(string_at(&json, "/error/message")?, "Invalid JSON body");
    }
    Ok(())
}

#[actix_rt::test]
async fn media_provider_posts_return_json_errors_without_sse_when_unwired() -> TestResult {
    // Given: upstream media-provider pages send valid provider test requests, including stream hints.
    //
    // Providers that genuinely expose the service reach credential lookup and
    // report 503 (no reachable credential store in this test); providers with no
    // such endpoint report 501. Both must be JSON, never SSE.
    let cases = [
        (
            "/v1/embeddings",
            r#"{"model":"openai/text-embedding-3-small","input":"hello","stream":true}"#,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            "/v1/audio/speech",
            r#"{"model":"openai/tts-1","input":"hello","stream":true}"#,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            "/v1/images/generations",
            r#"{"model":"openai/dall-e-3","prompt":"hello","stream":true}"#,
            StatusCode::SERVICE_UNAVAILABLE,
        ),
        (
            "/v1/search",
            r#"{"provider":"unsupported-search","query":"hello","stream":true}"#,
            StatusCode::NOT_IMPLEMENTED,
        ),
        (
            "/v1/web/fetch",
            r#"{"provider":"unsupported-fetch","url":"https://example.com","stream":true}"#,
            StatusCode::NOT_IMPLEMENTED,
        ),
    ];

    // When: each media-provider execution endpoint is invoked.
    for (route, body, expected_status) in cases {
        let response = request(Method::POST, route, body).await?;
        let json = parse_json(&response.body)?;

        // Then: the answer is a structured JSON error envelope, never SSE.
        assert_eq!(response.status, expected_status, "{route}");
        assert!(
            response.content_type.starts_with("application/json"),
            "{route}"
        );
        assert!(!response.content_type.starts_with("text/event-stream"));
        assert!(!response.body.contains("data: [DONE]"));
        assert!(
            !string_at(&json, "/error/message")?.is_empty(),
            "{route} must carry an error message"
        );
    }

    let voices = request(Method::GET, "/v1/audio/voices", "").await?;
    let voices_json = parse_json(&voices.body)?;

    assert_eq!(voices.status, StatusCode::OK);
    assert!(voices.content_type.starts_with("application/json"));
    assert_eq!(string_at(&voices_json, "/object")?, "list");
    assert!(
        voices_json
            .pointer("/data")
            .and_then(Value::as_array)
            .is_some_and(Vec::is_empty)
    );
    Ok(())
}

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}
