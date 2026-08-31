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

struct StreamCase {
    route: &'static str,
    expected_event: Option<&'static str>,
    code_pointer: &'static str,
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

fn sse_json_frame(body: &str) -> TestResult<Value> {
    let payload = body
        .lines()
        .find_map(|line| line.strip_prefix("data: "))
        .filter(|payload| payload.starts_with('{'))
        .ok_or_else(|| test_error("missing structured SSE JSON frame"))?;
    Ok(serde_json::from_str(payload)?)
}

fn has_final_done_frame(body: &str) -> bool {
    body.split("\n\n")
        .filter(|frame| !frame.is_empty())
        .last()
        .is_some_and(|frame| frame == "data: [DONE]")
}

fn assert_no_html_or_panic_fallback(route: &str, body: &str) {
    let lower_body = body.to_ascii_lowercase();
    assert!(!lower_body.contains("<!doctype"), "{route}");
    assert!(!lower_body.contains("<html"), "{route}");
    assert!(!lower_body.contains("panicked"), "{route}");
}

#[actix_rt::test]
async fn translator_stream_routes_keep_provider_execution_sse_failures() -> TestResult {
    // Given: translator-adjacent chat endpoints receive explicit streaming requests.
    let body = r#"{"model":"openai/gpt-5","stream":true,"messages":[{"role":"user","content":"translate hello"}]}"#;
    let cases = [
        StreamCase {
            route: "/v1/chat/completions",
            expected_event: None,
            code_pointer: "/error/code",
        },
        StreamCase {
            route: "/v1/responses",
            expected_event: Some("event: response.failed"),
            code_pointer: "/response/error/code",
        },
        StreamCase {
            route: "/v1/messages",
            expected_event: None,
            code_pointer: "/error/code",
        },
    ];

    // When: each route reaches the current provider-execution fallback.
    for case in cases {
        let response = request(Method::POST, case.route, body).await?;
        let frame = sse_json_frame(&response.body)?;

        // Then: every stream keeps a structured text/event-stream failure and final DONE.
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
        if let Some(expected_event) = case.expected_event {
            assert!(response.body.contains(expected_event), "{}", case.route);
        }
        assert_eq!(
            frame.pointer(case.code_pointer),
            Some(&Value::String("service_unavailable".to_owned())),
            "{}",
            case.route
        );
        assert!(has_final_done_frame(&response.body), "{}", case.route);
        assert_no_html_or_panic_fallback(case.route, &response.body);
    }
    Ok(())
}

fn test_error(message: impl Into<String>) -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::other(message.into()))
}
