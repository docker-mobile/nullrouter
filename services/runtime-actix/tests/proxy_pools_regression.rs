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

#[derive(Debug)]
struct SseFrame<'a> {
    event: Option<&'a str>,
    data: &'a str,
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

fn parse_sse_frames(body: &str) -> Vec<SseFrame<'_>> {
    body.split("\n\n")
        .filter_map(|chunk| {
            let mut event = None;
            let mut data = None;

            for line in chunk.lines() {
                if let Some(value) = line.strip_prefix("event: ") {
                    event = Some(value);
                } else if let Some(value) = line.strip_prefix("data: ") {
                    data = Some(value);
                }
            }

            data.map(|payload| SseFrame {
                event,
                data: payload,
            })
        })
        .collect()
}

fn first_json_frame<'a>(frames: &'a [SseFrame<'a>]) -> TestResult<(&'a SseFrame<'a>, Value)> {
    let frame = frames
        .iter()
        .find(|frame| frame.data.starts_with('{'))
        .ok_or_else(|| test_error("missing JSON SSE data frame"))?;
    Ok((frame, serde_json::from_str(frame.data)?))
}

fn has_done_frame(frames: &[SseFrame<'_>]) -> bool {
    frames.iter().any(|frame| frame.data == "[DONE]")
}

fn assert_sse_response(route: &str, response: &RuntimeResponse) {
    assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE, "{route}");
    assert!(
        response.content_type.starts_with("text/event-stream"),
        "{route}"
    );
    let lowercase_body = response.body.to_ascii_lowercase();
    assert!(!lowercase_body.contains("<html"), "{route}");
    assert!(!lowercase_body.contains("<!doctype"), "{route}");
}

#[actix_rt::test]
async fn proxy_pools_parity_keeps_runtime_stream_failures_structured() -> TestResult {
    // Given: proxy-pool dashboard/state work must not change runtime provider failure streams.
    let body = r#"{"model":"openai/gpt-5","stream":true,"messages":[]}"#;

    // When: OpenAI-compatible runtime stream endpoints are invoked.
    let chat = request(Method::POST, "/v1/chat/completions", body).await?;
    let responses = request(Method::POST, "/v1/responses", body).await?;
    let messages = request(Method::POST, "/v1/messages", body).await?;

    // Then: chat completions and messages keep OpenAI data frames plus [DONE].
    for (route, response) in [("/v1/chat/completions", &chat), ("/v1/messages", &messages)] {
        assert_sse_response(route, response);

        let frames = parse_sse_frames(&response.body);
        let (frame, event) = first_json_frame(&frames)?;
        assert_eq!(frame.event, None, "{route}");
        assert!(
            event
                .pointer("/error/message")
                .and_then(Value::as_str)
                .is_some_and(|message| !message.is_empty()),
            "{route} must carry an error message"
        );
        assert!(has_done_frame(&frames), "{route}");
    }

    // Then: Responses API streams keep the response.failed event frame plus [DONE].
    assert_sse_response("/v1/responses", &responses);
    let frames = parse_sse_frames(&responses.body);
    let (frame, event) = first_json_frame(&frames)?;
    assert_eq!(frame.event, Some("response.failed"));
    assert_eq!(
        event.pointer("/type"),
        Some(&Value::String("response.failed".to_owned()))
    );
    assert_eq!(
        event.pointer("/response/error/code"),
        Some(&Value::String("service_unavailable".to_owned()))
    );
    assert!(has_done_frame(&frames));
    Ok(())
}

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}
