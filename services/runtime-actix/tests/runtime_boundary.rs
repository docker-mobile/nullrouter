#![allow(clippy::future_not_send)]

use actix_web::{
    App,
    body::to_bytes,
    http::{Method, StatusCode, header},
    test, web,
};
use serde_json::Value;

use nullrouter_runtime::{Runtime, app_config, configure};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A closed loopback port: credential lookup fails deterministically as
/// "state unavailable", so these route-shape tests need no state service.
const UNREACHABLE_STATE_ADDR: &str = "127.0.0.1:1";

async fn request(method: Method, uri: &str, body: &str) -> TestResult<RuntimeResponse> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(Runtime::with_state_addr(
                UNREACHABLE_STATE_ADDR,
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

struct RuntimeResponse {
    status: StatusCode,
    content_type: String,
    body: String,
}

fn parse_json(body: &str) -> TestResult<Value> {
    Ok(serde_json::from_str(body)?)
}

fn error_message(json: &Value) -> TestResult<&str> {
    json.pointer("/error/message")
        .and_then(Value::as_str)
        .ok_or_else(|| test_error("missing error.message"))
}

fn first_sse_json(body: &str) -> TestResult<Value> {
    let payload = body
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .filter(|payload| payload.starts_with('{'))
        .ok_or_else(|| test_error("missing JSON SSE data frame"))?;
    parse_json(payload)
}

fn has_done_frame(body: &str) -> bool {
    body.lines().any(|line| line == "data: [DONE]")
}

#[actix_rt::test]
async fn chat_routes_return_structured_bad_requests_when_json_or_model_is_invalid() -> TestResult {
    // Given: chat-style endpoints parse JSON at the HTTP boundary.
    let routes = [
        "/v1/chat/completions",
        "/v1/responses",
        "/v1/messages",
        "/v1/api/chat",
    ];

    // When: malformed JSON and missing model bodies are posted.
    for route in routes {
        let malformed = request(Method::POST, route, "{").await?;
        let malformed_json = parse_json(&malformed.body)?;
        let missing = request(Method::POST, route, "{}").await?;
        let missing_json = parse_json(&missing.body)?;

        // Then: the response is a JSON 400 envelope, not an Actix HTML error.
        assert_eq!(malformed.status, StatusCode::BAD_REQUEST, "{route}");
        assert!(malformed.content_type.starts_with("application/json"));
        assert_eq!(error_message(&malformed_json)?, "Invalid JSON body");
        assert_eq!(missing.status, StatusCode::BAD_REQUEST, "{route}");
        assert_eq!(
            error_message(&missing_json)?,
            "Missing required field: model"
        );
    }
    Ok(())
}

#[actix_rt::test]
async fn chat_routes_return_json_or_sse_provider_execution_failures() -> TestResult {
    // Given: chat-style endpoints receive valid provider-backed requests.
    let json_body = r#"{"model":"openai/gpt-5","messages":[]}"#;
    let stream_body = r#"{"model":"openai/gpt-5","stream":true,"messages":[]}"#;

    // When: non-stream and stream variants are posted.
    let json = request(Method::POST, "/v1/chat/completions", json_body).await?;
    let stream = request(Method::POST, "/v1/chat/completions", stream_body).await?;
    let responses = request(Method::POST, "/v1/responses", stream_body).await?;
    let messages = request(Method::POST, "/v1/messages", stream_body).await?;

    // Then: non-stream uses JSON and streams use OpenAI-compatible SSE frames.
    assert_eq!(json.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(json.content_type.starts_with("application/json"));

    for (route, response) in [
        ("/v1/chat/completions", &stream),
        ("/v1/messages", &messages),
    ] {
        let event = first_sse_json(&response.body)?;
        assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE, "{route}");
        assert!(
            response.content_type.starts_with("text/event-stream"),
            "{route}"
        );
        assert!(
            event
                .pointer("/error/message")
                .and_then(Value::as_str)
                .is_some_and(|message| !message.is_empty()),
            "{route} must carry an error message"
        );
        assert!(has_done_frame(&response.body), "{route}");
    }

    let responses_event = first_sse_json(&responses.body)?;
    assert_eq!(responses.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(responses.content_type.starts_with("text/event-stream"));
    assert!(responses.body.contains("event: response.failed"));
    assert_eq!(
        responses_event.pointer("/type"),
        Some(&Value::String("response.failed".to_owned()))
    );
    assert!(
        responses_event
            .pointer("/response/error/message")
            .and_then(Value::as_str)
            .is_some_and(|message| !message.is_empty())
    );
    assert!(has_done_frame(&responses.body));
    Ok(())
}

#[actix_rt::test]
async fn provider_routes_return_json_bad_requests_when_body_is_malformed() -> TestResult {
    // Given: provider-backed endpoints accept structured JSON input in this slice.
    let routes = [
        "/v1/embeddings",
        "/v1/images/generations",
        "/v1/audio/speech",
        "/v1/audio/transcriptions",
        "/v1/search",
        "/v1/web/fetch",
        "/v1/messages/count_tokens",
        "/v1/responses/compact",
        "/v1beta/models/gemini/gemini-2.5-pro:generateContent",
    ];

    // When: malformed JSON is posted.
    for route in routes {
        let response = request(Method::POST, route, "{").await?;
        let json = parse_json(&response.body)?;

        // Then: every boundary returns a structured JSON 400 response.
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{route}");
        assert!(
            response.content_type.starts_with("application/json"),
            "{route}"
        );
        assert_eq!(error_message(&json)?, "Invalid JSON body", "{route}");
    }
    Ok(())
}

#[actix_rt::test]
async fn provider_routes_validate_required_fields_before_execution() -> TestResult {
    // Given: valid JSON bodies omit endpoint-specific required fields.
    let cases = [
        ("/v1/embeddings", "{}", "Missing required field: model"),
        (
            "/v1/images/generations",
            r#"{"model":"openai/dall-e-3"}"#,
            "Missing required field: prompt",
        ),
        (
            "/v1/audio/speech",
            r#"{"model":"openai/tts-1"}"#,
            "Missing required field: input",
        ),
        (
            "/v1/audio/transcriptions",
            r#"{"model":"openai/whisper-1"}"#,
            "Missing required field: file",
        ),
        (
            "/v1/search",
            "{}",
            "Missing required field: provider (or model)",
        ),
        (
            "/v1/web/fetch",
            r#"{"provider":"firecrawl"}"#,
            "Missing required field: url",
        ),
    ];

    // When: each request crosses the runtime boundary.
    for (route, body, expected) in cases {
        let response = request(Method::POST, route, body).await?;
        let json = parse_json(&response.body)?;

        // Then: validation errors are explicit and structured.
        assert_eq!(response.status, StatusCode::BAD_REQUEST, "{route}");
        assert_eq!(error_message(&json)?, expected, "{route}");
    }
    Ok(())
}

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}
