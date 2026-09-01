#![allow(clippy::future_not_send)]

use actix_web::{
    App,
    body::to_bytes,
    http::{StatusCode, header},
    test, web,
};
use nullrouter_api::{AppConfig, RuntimeClient, StateClient, TunnelManager, configure};

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
            .app_data(web::Data::new(TunnelManager::new()))
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

#[actix_rt::test]
async fn chat_stream_true_returns_openai_sse_error_envelope_when_provider_execution_is_unimplemented()
-> TestResult {
    // Given: an OpenAI chat-completions request explicitly asks for streaming.
    let payload = r#"{"model":"openai/gpt-5","messages":[],"stream":true}"#;

    // When: the Rust port reaches the provider execution boundary.
    let (status, content_type, text) = post_json("/v1/chat/completions", payload).await?;

    // Then: the unavailable provider execution is framed as SSE, not JSON.
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert!(content_type.starts_with("text/event-stream"));
    assert!(text.starts_with("data: {\"error\""));
    assert!(text.contains("\"code\":\"provider_execution_unimplemented\""));
    assert!(text.ends_with("data: [DONE]\n\n"));
    Ok(())
}

#[actix_rt::test]
async fn responses_and_messages_default_to_streaming_when_stream_is_omitted() -> TestResult {
    // Given: upstream treats /v1/responses and /v1/messages as chat entrypoints
    // with path-forced request formats and stream enabled unless stream=false.
    let cases = [
        (
            "/v1/responses",
            r#"{"model":"openai/gpt-5","input":"hello"}"#,
        ),
        (
            "/v1/messages",
            r#"{"model":"anthropic/claude-sonnet-4.5","max_tokens":128,"messages":[{"role":"user","content":[{"type":"text","text":"hello"}]}]}"#,
        ),
    ];

    for (uri, payload) in cases {
        // When: the endpoint receives its native request shape.
        let (status, content_type, text) = post_json(uri, payload).await?;

        // Then: the current provider stub still preserves the upstream stream decision.
        assert_eq!(status, StatusCode::NOT_IMPLEMENTED, "{uri}");
        assert!(content_type.starts_with("text/event-stream"), "{uri}");
        assert!(text.contains("\"stream\":true"), "{uri}");
    }
    Ok(())
}

#[actix_rt::test]
async fn responses_streaming_errors_use_responses_api_failed_event() -> TestResult {
    // Given: a Responses API request explicitly asks for streaming.
    let payload = r#"{"model":"openai/gpt-5","input":"hello","stream":true}"#;

    // When: the Rust port reaches the provider execution boundary.
    let (status, content_type, text) = post_json("/v1/responses", payload).await?;

    // Then: the error is framed as a Responses API terminal event.
    assert_eq!(status, StatusCode::NOT_IMPLEMENTED);
    assert!(content_type.starts_with("text/event-stream"));
    assert!(text.starts_with("event: response.failed\ndata: {"));
    assert!(text.contains("\"type\":\"response.failed\""));
    assert!(text.contains("\"status\":\"failed\""));
    assert!(text.contains("\"code\":\"provider_execution_unimplemented\""));
    assert!(text.ends_with("data: [DONE]\n\n"));
    Ok(())
}
