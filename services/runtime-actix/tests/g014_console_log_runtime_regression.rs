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

struct RuntimeResponse {
    status: StatusCode,
    content_type: String,
    body: String,
}

struct StreamCase {
    route: &'static str,
    expected_event: Option<&'static str>,
    code_pointer: &'static str,
}

struct SseFrame<'a> {
    event: Option<&'a str>,
    data: &'a str,
}

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
        .ok_or_else(|| test_error("missing structured SSE JSON frame"))?;
    Ok((frame, serde_json::from_str(frame.data)?))
}

fn has_done_frame(frames: &[SseFrame<'_>]) -> bool {
    frames.iter().any(|frame| frame.data == "[DONE]")
}

fn assert_no_html_or_panic_fallback(route: &str, body: &str) {
    let lower_body = body.to_ascii_lowercase();
    assert!(!lower_body.contains("<!doctype"), "{route}");
    assert!(!lower_body.contains("<html"), "{route}");
    assert!(!lower_body.contains("panicked"), "{route}");
}

#[actix_rt::test]
async fn g014_console_log_models_route_returns_structured_json_when_requested() -> TestResult {
    // Given: console-log parity smoke can request runtime model discovery.

    // When: the OpenAI-compatible model list route is requested.
    let response = request(Method::GET, "/v1/models", "").await?;
    let json: Value = serde_json::from_str(&response.body)?;

    // Then: the runtime returns structured JSON, not an HTML fallback or panic page.
    assert_eq!(response.status, StatusCode::OK);
    assert!(
        response.content_type.starts_with("application/json"),
        "/v1/models"
    );
    assert_eq!(json.get("object"), Some(&Value::String("list".to_owned())));
    assert!(
        json.get("data")
            .and_then(Value::as_array)
            .is_some_and(|models| !models.is_empty())
    );
    assert_no_html_or_panic_fallback("/v1/models", &response.body);
    Ok(())
}

#[actix_rt::test]
async fn g014_console_log_stream_routes_return_structured_provider_failures() -> TestResult {
    // Given: console-log parity smoke can hit runtime stream routes after model discovery.
    let body =
        r#"{"model":"openai/gpt-5","stream":true,"messages":[{"role":"user","content":"hello"}]}"#;
    let cases = [
        StreamCase {
            route: "/v1/chat/completions",
            expected_event: None,
            code_pointer: "/error/code",
        },
        StreamCase {
            route: "/v1/responses",
            expected_event: Some("response.failed"),
            code_pointer: "/response/error/code",
        },
    ];

    // When: stream=true requests reach the provider-execution fallback.
    for case in cases {
        let response = request(Method::POST, case.route, body).await?;
        let frames = parse_sse_frames(&response.body);
        let (frame, json) = first_json_frame(&frames)?;

        // Then: each route returns structured SSE provider failure frames and [DONE].
        assert_eq!(
            response.status,
            StatusCode::SERVICE_UNAVAILABLE,
            "{}",
            case.route
        );
        assert!(
            response.content_type.starts_with("text/event-stream"),
            "{}",
            case.route
        );
        assert_eq!(frame.event, case.expected_event, "{}", case.route);
        assert_eq!(
            json.pointer(case.code_pointer),
            Some(&Value::String("service_unavailable".to_owned())),
            "{}",
            case.route
        );
        assert!(has_done_frame(&frames), "{}", case.route);
        assert_no_html_or_panic_fallback(case.route, &response.body);
    }
    Ok(())
}

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}
