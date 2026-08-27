#![allow(clippy::future_not_send)]

use actix_web::{
    App,
    body::to_bytes,
    http::{Method, StatusCode, header},
    test, web,
};
use serde_json::Value;

use nullrouter_api::{AppConfig, RuntimeClient, StateClient, configure};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A closed loopback port: usage reads fall back to the zeroed shape,
/// so these parity tests need no state service.
const UNREACHABLE_STATE_ADDR: &str = "127.0.0.1:1";

const fn app_config() -> AppConfig {
    AppConfig::new("0.5.20")
}

async fn post_json(uri: &str, payload: &'static str) -> TestResult<(StatusCode, String, String)> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(StateClient::new(UNREACHABLE_STATE_ADDR)))
            .app_data(web::Data::new(RuntimeClient::new(UNREACHABLE_STATE_ADDR)))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::post()
        .uri(uri)
        .insert_header((header::CONTENT_TYPE, "application/json"))
        .set_payload(payload)
        .to_request();

    let res = test::call_service(&app, req).await;
    let status = res.status();
    let content_type = res
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let body = to_bytes(res.into_body()).await?;
    let text = std::str::from_utf8(&body)?.to_owned();

    Ok((status, content_type, text))
}

async fn post_json_value(uri: &str, payload: &'static str) -> TestResult<(StatusCode, Value)> {
    let (status, _content_type, text) = post_json(uri, payload).await?;
    let json = serde_json::from_str(&text)?;

    Ok((status, json))
}

async fn request_empty(
    method: Method,
    uri: &str,
) -> TestResult<(StatusCode, Vec<(String, String)>)> {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(app_config()))
            .app_data(web::Data::new(StateClient::new(UNREACHABLE_STATE_ADDR)))
            .app_data(web::Data::new(RuntimeClient::new(UNREACHABLE_STATE_ADDR)))
            .configure(configure),
    )
    .await;
    let req = test::TestRequest::default()
        .method(method)
        .uri(uri)
        .to_request();
    let res = test::call_service(&app, req).await;
    let status = res.status();
    let headers = res
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_owned(), value.to_owned()))
        })
        .collect();

    Ok((status, headers))
}

fn field<'a>(json: &'a Value, name: &str) -> TestResult<&'a Value> {
    json.get(name)
        .ok_or_else(|| test_error(format!("missing field {name}")))
}

fn nested_field<'a>(json: &'a Value, first: &str, second: &str) -> TestResult<&'a Value> {
    field(field(json, first)?, second)
}

fn has_cors_header(headers: &[(String, String)]) -> bool {
    headers
        .iter()
        .any(|(name, value)| name == "access-control-allow-origin" && value == "*")
}

#[actix_rt::test]
async fn dashboard_chat_stream_true_returns_sse_provider_execution_frame() -> TestResult {
    // Given: Basic Chat sends an OpenAI-style streaming chat-completions body.
    let payload =
        r#"{"model":"openai/gpt-5","messages":[{"role":"user","content":"hello"}],"stream":true}"#;

    // When: the dashboard route reaches the unwired provider execution boundary.
    let (status, content_type, text) =
        post_json("/api/dashboard/chat/completions", payload).await?;

    // Then: the failure is framed as provider SSE, not a dashboard HTML or route fallback.
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert!(content_type.starts_with("text/event-stream"));
    assert!(text.starts_with("data: {\"error\""));
    assert!(text.contains("\"code\":\"provider_execution_unimplemented\""));
    assert!(text.contains("\"model\":\"openai/gpt-5\""));
    assert!(text.contains("\"stream\":true"));
    assert!(text.ends_with("data: [DONE]\n\n"));
    assert!(!text.contains("<!doctype html>"));
    Ok(())
}

#[actix_rt::test]
async fn dashboard_chat_stream_false_returns_json_provider_execution_stub() -> TestResult {
    // Given: Basic Chat sends a non-streaming chat-completions body.
    let payload =
        r#"{"model":"openai/gpt-5","messages":[{"role":"user","content":"hello"}],"stream":false}"#;

    // When: provider execution would normally run.
    let (status, json) = post_json_value("/api/dashboard/chat/completions", payload).await?;

    // Then: the API is explicit that provider execution is not wired.
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert_eq!(nested_field(&json, "error", "type")?, "not_implemented");
    assert_eq!(
        nested_field(&json, "error", "code")?,
        "provider_execution_unimplemented"
    );
    assert_eq!(nested_field(&json, "error", "model")?, "openai/gpt-5");
    assert_eq!(nested_field(&json, "error", "stream")?, false);
    Ok(())
}

#[actix_rt::test]
async fn dashboard_chat_rejects_bad_json_missing_model_and_missing_messages() -> TestResult {
    // Given: Basic Chat callers can send malformed or incomplete bodies.
    let cases = [
        ("{", "Invalid JSON body"),
        (
            r#"{"messages":[{"role":"user","content":"hello"}],"stream":false}"#,
            "Missing required field: model",
        ),
        (
            r#"{"model":"openai/gpt-5","stream":false}"#,
            "Missing required field: messages",
        ),
        (
            r#"{"model":"openai/gpt-5","messages":{},"stream":false}"#,
            "Invalid JSON body",
        ),
    ];

    for (payload, message) in cases {
        // When: the dashboard chat route receives the invalid body.
        let (status, json) = post_json_value("/api/dashboard/chat/completions", payload).await?;

        // Then: each failure is a structured JSON 400.
        assert_eq!(status, StatusCode::BAD_REQUEST, "{payload}");
        assert_eq!(
            nested_field(&json, "error", "type")?,
            "invalid_request_error",
            "{payload}"
        );
        assert_eq!(
            nested_field(&json, "error", "message")?,
            message,
            "{payload}"
        );
    }
    Ok(())
}

#[actix_rt::test]
async fn dashboard_chat_options_returns_no_content_with_cors_headers() -> TestResult {
    // Given: the dashboard browser can preflight the Basic Chat POST route.

    // When: OPTIONS is requested for the chat-completions endpoint.
    let (status, headers) =
        request_empty(Method::OPTIONS, "/api/dashboard/chat/completions").await?;

    // Then: the route answers no-content and preserves shared CORS headers.
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(has_cors_header(&headers));
    Ok(())
}

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}
